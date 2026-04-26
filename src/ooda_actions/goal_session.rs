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
pub(super) fn assess_only_outcome(
    action: &PlannedAction,
    board: &mut GoalBoard,
    goal_id: &str,
    assessment: &str,
    progress_pct: u8,
) -> ActionOutcome {
    let new_progress = if progress_pct >= 100 {
        GoalProgress::Completed
    } else if progress_pct == 0 {
        GoalProgress::NotStarted
    } else {
        GoalProgress::InProgress {
            percent: progress_pct as u32,
        }
    };

    let assessment_short = truncate_for_outcome(assessment);

    match update_goal_progress(board, goal_id, new_progress) {
        Ok(()) => {
            eprintln!(
                "[simard] OODA goal-action assess_only for '{}': {} (progress={}%)",
                goal_id, assessment_short, progress_pct,
            );
            let detail = format!(
                "assess_only: {} (progress={}%, goal '{}')",
                assessment_short, progress_pct, goal_id,
            );
            make_outcome(action, true, detail)
        }
        Err(e) => {
            eprintln!(
                "[simard] OODA goal-action assess_only FAILED to update progress for '{}': {} (assessment='{}', progress={}%)",
                goal_id, e, assessment_short, progress_pct,
            );
            let detail = format!(
                "assess_only failed: update_goal_progress error for goal '{}': {} (assessment='{}', progress={}%)",
                goal_id, e, assessment_short, progress_pct,
            );
            make_outcome(action, false, detail)
        }
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
                    let outcome = assess_only_outcome(
                        action,
                        &mut state.active_goals,
                        &goal.id,
                        assessment,
                        progress_pct,
                    );
                    GoalSessionResult {
                        outcome,
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
                Some(GoalAction::GhIssueCreate {
                    ref title,
                    ref body,
                    ref repo,
                    ref labels,
                }) => {
                    let repo_arg = repo.as_deref().unwrap_or("rysweet/Simard");
                    let result = dispatch_gh_issue_create(repo_arg, title, body, labels);
                    let detail = match result {
                        Ok(ref url) => format!(
                            "gh_issue_create succeeded for goal '{}': {} (title={})",
                            goal.id,
                            url,
                            truncate_for_outcome(title),
                        ),
                        Err(ref e) => format!(
                            "gh_issue_create FAILED for goal '{}': {} (title={})",
                            goal.id,
                            e,
                            truncate_for_outcome(title),
                        ),
                    };
                    eprintln!("[simard] OODA goal-action {detail}");
                    GoalSessionResult {
                        outcome: make_outcome(action, result.is_ok(), detail),
                        action: parsed,
                    }
                }
                Some(GoalAction::GhIssueComment {
                    issue,
                    ref body,
                    ref repo,
                }) => {
                    let repo_arg = repo.as_deref().unwrap_or("rysweet/Simard");
                    let result = dispatch_gh_issue_comment(repo_arg, issue, body);
                    let detail = match result {
                        Ok(ref url) => format!(
                            "gh_issue_comment succeeded for goal '{}': issue #{issue} {url}",
                            goal.id,
                        ),
                        Err(ref e) => format!(
                            "gh_issue_comment FAILED for goal '{}': issue #{issue}: {e}",
                            goal.id,
                        ),
                    };
                    eprintln!("[simard] OODA goal-action {detail}");
                    GoalSessionResult {
                        outcome: make_outcome(action, result.is_ok(), detail),
                        action: parsed,
                    }
                }
                Some(GoalAction::GhIssueClose {
                    issue,
                    ref comment,
                    ref repo,
                }) => {
                    let repo_arg = repo.as_deref().unwrap_or("rysweet/Simard");
                    let result = dispatch_gh_issue_close(repo_arg, issue, comment.as_deref());
                    let detail = match result {
                        Ok(()) => format!(
                            "gh_issue_close succeeded for goal '{}': closed issue #{issue}",
                            goal.id,
                        ),
                        Err(ref e) => format!(
                            "gh_issue_close FAILED for goal '{}': issue #{issue}: {e}",
                            goal.id,
                        ),
                    };
                    eprintln!("[simard] OODA goal-action {detail}");
                    GoalSessionResult {
                        outcome: make_outcome(action, result.is_ok(), detail),
                        action: parsed,
                    }
                }
                Some(GoalAction::GhPrComment {
                    pr,
                    ref body,
                    ref repo,
                }) => {
                    let repo_arg = repo.as_deref().unwrap_or("rysweet/Simard");
                    let result = dispatch_gh_pr_comment(repo_arg, pr, body);
                    let detail = match result {
                        Ok(ref url) => format!(
                            "gh_pr_comment succeeded for goal '{}': pr #{pr} {url}",
                            goal.id,
                        ),
                        Err(ref e) => {
                            format!("gh_pr_comment FAILED for goal '{}': pr #{pr}: {e}", goal.id,)
                        }
                    };
                    eprintln!("[simard] OODA goal-action {detail}");
                    GoalSessionResult {
                        outcome: make_outcome(action, result.is_ok(), detail),
                        action: parsed,
                    }
                }
                None => {
                    // FAIL LOUD per PHILOSOPHY.md: when the LLM returns prose
                    // instead of the required JSON object, this is a planning
                    // failure. We do NOT fall back to PROGRESS-line scraping —
                    // that masked broken planning for months. Surface the
                    // failure so the cooldown machinery can demote this goal.
                    let raw = truncate_for_outcome(&outcome.execution_summary);
                    eprintln!(
                        "[simard] OODA goal-action PARSE FAILED for '{}': LLM returned non-JSON response: {}",
                        goal.id, raw,
                    );
                    let detail = format!(
                        "goal-action parse failed for goal '{}': LLM did not emit a recognised JSON action (one of spawn_engineer / noop / assess_only / gh_issue_create / gh_issue_comment / gh_issue_close / gh_pr_comment). Raw response head: {}",
                        goal.id, raw,
                    );
                    GoalSessionResult {
                        outcome: make_outcome(action, false, detail),
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

// ─── Native gh CLI dispatchers ─────────────────────────────────────────
//
// Orchestrator-owned actions: Simard executes these directly instead of
// spawning an engineer subprocess. This is the right boundary because each
// action is a single bounded gh CLI call with a deterministic outcome —
// engineer subprocesses are reserved for code-mutating work that needs the
// inspect→plan→execute→verify→persist pipeline.

/// Maximum bytes accepted from any single argv string passed to `gh`.
/// Keeps a malformed/runaway LLM response from constructing a giant CLI.
const GH_ARG_MAX_BYTES: usize = 32 * 1024;

fn run_gh(args: &[&str]) -> Result<String, String> {
    for a in args {
        if a.len() > GH_ARG_MAX_BYTES {
            return Err(format!(
                "gh argument exceeds {GH_ARG_MAX_BYTES} bytes (got {})",
                a.len()
            ));
        }
    }
    let output = std::process::Command::new("gh")
        .args(args)
        .output()
        .map_err(|e| format!("failed to execute gh: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!(
            "gh exited with status {}: {stderr}",
            output.status.code().unwrap_or(-1)
        ))
    }
}

/// Look up an open issue in `repo` whose title exactly matches `title`.
/// Returns `Ok(Some(number))` if a duplicate exists, `Ok(None)` otherwise.
/// Errors are non-fatal — we treat lookup failure as "no duplicate found"
/// and let the caller proceed (failing closed on dedup would block all
/// issue creation if the gh search API hiccups).
fn find_duplicate_open_issue(repo: &str, title: &str) -> Result<Option<u64>, String> {
    let search = format!("\"{}\" in:title", title.replace('"', "\\\""));
    let json = run_gh(&[
        "issue",
        "list",
        "--repo",
        repo,
        "--state",
        "open",
        "--search",
        &search,
        "--json",
        "number,title",
        "--limit",
        "10",
    ])?;
    #[derive(serde::Deserialize)]
    struct Hit {
        number: u64,
        title: String,
    }
    let hits: Vec<Hit> =
        serde_json::from_str(&json).map_err(|e| format!("dedup parse failed: {e}"))?;
    let target = title.trim();
    Ok(hits
        .into_iter()
        .find(|h| h.title.trim() == target)
        .map(|h| h.number))
}

fn dispatch_gh_issue_create(
    repo: &str,
    title: &str,
    body: &str,
    labels: &[String],
) -> Result<String, String> {
    // Pre-flight dedup: refuse to file a second open issue with the same
    // title. The OODA daemon repeatedly proposed identical titles
    // (#1178-1183 were six dupes of #1177; #1247-1250 were four dupes of
    // each other). Title-hash check is cheap and stops the worst case.
    match find_duplicate_open_issue(repo, title) {
        Ok(Some(existing)) => {
            return Err(format!(
                "duplicate of open issue #{existing}: title \"{title}\" already exists"
            ));
        }
        Ok(None) => {}
        Err(e) => {
            // Non-fatal: log and proceed. We'd rather risk an occasional
            // dupe than block all issue creation on a search-API blip.
            eprintln!("[simard] dedup lookup failed (proceeding): {e}");
        }
    }

    let mut args: Vec<&str> = vec![
        "issue", "create", "--repo", repo, "--title", title, "--body", body,
    ];
    let label_csv;
    let sanitized_labels: Vec<String> = labels
        .iter()
        .map(|l| l.trim().to_string())
        .filter(|l| is_plausible_label(l))
        .collect();
    if !sanitized_labels.is_empty() {
        label_csv = sanitized_labels.join(",");
        args.push("--label");
        args.push(&label_csv);
    }
    run_gh(&args)
}

/// Filter labels that are obviously bogus (placeholders, ellipses, control chars, empty).
/// Real labels here are short kebab-case-or-spaced strings; LLM occasionally emits
/// `"..."` or `".…"` (literal ellipsis) from truncated examples in the prompt.
pub(super) fn is_plausible_label(label: &str) -> bool {
    if label.is_empty() || label.len() > 50 {
        return false;
    }
    // Reject pure-punctuation placeholders the LLM tends to emit (`...`, `.…`, `…`).
    if label
        .chars()
        .all(|c| matches!(c, '.' | '…' | '-' | '_' | ' '))
    {
        return false;
    }
    // Require at least one alphanumeric character.
    label.chars().any(|c| c.is_alphanumeric())
}

fn dispatch_gh_issue_comment(repo: &str, issue: u64, body: &str) -> Result<String, String> {
    let issue_str = issue.to_string();
    run_gh(&[
        "issue", "comment", &issue_str, "--repo", repo, "--body", body,
    ])
}

fn dispatch_gh_issue_close(repo: &str, issue: u64, comment: Option<&str>) -> Result<(), String> {
    let issue_str = issue.to_string();
    if let Some(body) = comment
        && !body.trim().is_empty()
    {
        let _ = run_gh(&[
            "issue", "comment", &issue_str, "--repo", repo, "--body", body,
        ])?;
    }
    let _ = run_gh(&["issue", "close", &issue_str, "--repo", repo])?;
    Ok(())
}

fn dispatch_gh_pr_comment(repo: &str, pr: u64, body: &str) -> Result<String, String> {
    let pr_str = pr.to_string();
    run_gh(&["pr", "comment", &pr_str, "--repo", repo, "--body", body])
}

