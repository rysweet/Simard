//! Goal-session "advance" + "assess-only" outcome computation.

use crate::error::SimardResult;
use crate::goal_curation::{GoalBoard, GoalProgress, update_goal_progress};
use crate::ooda_loop::{ActionOutcome, OodaState, PlannedAction};

use super::super::make_outcome;
use super::gh::{
    dispatch_gh_issue_close, dispatch_gh_issue_comment, dispatch_gh_issue_create,
    dispatch_gh_pr_comment, find_duplicate_open_issue,
};
use super::{
    GoalAction, GoalSessionResult, OUTCOME_TEXT_MAX, parse_goal_action, truncate_for_outcome,
};

pub(crate) fn assess_only_outcome(
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
pub(crate) fn advance_goal_with_session(
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
        include_str!("../../../prompt_assets/simard/goal_session_objective.md");

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
        include_str!("../../../prompt_assets/simard/goal_session_identity.md");
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
