//! Session-based goal advancement — delegates work to a base-type agent.

use crate::goal_curation::{GoalProgress, update_goal_progress};
use crate::ooda_loop::{ActionOutcome, OodaState, PlannedAction};

use super::make_outcome;
use super::verification::{assess_progress_from_outcome, verify_claimed_actions};

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
) -> ActionOutcome {
    use crate::base_types::BaseTypeTurnInput;
    use std::fmt::Write;

    let percent = match &goal.status {
        GoalProgress::InProgress { percent } => *percent,
        _ => 0,
    };

    // Gather fresh environment context so the agent sees current state.
    let env = crate::ooda_loop::gather_environment();

    // Build the objective in a single pre-sized buffer to avoid intermediate allocations.
    let mut objective = String::with_capacity(1024);
    let _ = write!(
        objective,
        "Goal '{}' ({}% complete): {}\n\n\
         Assess this goal's current status by:\n\
         1. Check the repository state, open issues, and recent commits to understand where things stand.\n\
         2. Decide whether this goal needs an amplihack coding session to make progress.\n\
         3. If work is needed: create a GitHub issue describing the specific task, then launch \
            `simard engineer` or `amplihack copilot` to handle it.\n\
         4. If the goal is already progressing or blocked, report the status without launching new work.\n\n\
         End your response with a PROGRESS line indicating your assessed completion percentage \
         (0-100), e.g.: PROGRESS: 45\n\n\
         Concrete commands you can use:\n\
         - Create a GitHub issue: `gh issue create --repo rysweet/Simard --title \"<title>\" --body \"<body>\"`\n\
         - Create a branch: `git checkout -b feat/<description>`\n\
         - Launch an amplihack coding session: `amplihack copilot` then type your task\n\
         - Run tests: `cargo test 2>&1 | tail -20`\n\
         - Check build: `cargo check 2>&1`\n\
         - Open a PR: `gh pr create --title \"<title>\" --body \"<body>\"`\n\
         - Check CI status: `gh run list --limit 5`\n\n\
         Environment context:\n- Git status: ",
        goal.id, percent, goal.description,
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

    let identity_context = "You are Simard, a PM architect who manages fleets of amplihack \
        coding sessions. You do NOT write code yourself. You assess goals, create GitHub \
        issues for specific work items, and delegate implementation to amplihack coding \
        agents (via `simard engineer` or `amplihack copilot`). Your job is to evaluate \
        what needs to happen, break it into actionable work, and orchestrate the right \
        agent to do it."
        .to_string();

    let input = BaseTypeTurnInput {
        objective,
        identity_context,
        prompt_preamble: String::new(),
    };

    match session.run_turn(input) {
        Ok(outcome) => {
            let new_progress = assess_progress_from_outcome(&outcome, &goal.status);

            // Verify claimed actions against reality.
            let verification = verify_claimed_actions(&outcome.execution_summary);
            let verified_count = verification.iter().filter(|v| v.verified).count();
            let claimed_count = verification.len();

            let _ = update_goal_progress(&mut state.active_goals, &goal.id, new_progress.clone());

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

            make_outcome(
                action,
                true,
                format!(
                    "goal '{}' assessed at {} via session (evidence={}, verified={}/{})",
                    goal.id,
                    new_progress,
                    outcome.evidence.len(),
                    verified_count,
                    claimed_count,
                ),
            )
        }
        Err(e) => make_outcome(
            action,
            false,
            format!("session run_turn failed for goal '{}': {e}", goal.id),
        ),
    }
}
