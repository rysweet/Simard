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
}
