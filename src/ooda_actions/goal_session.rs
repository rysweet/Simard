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

#[cfg(test)]
mod tests {
    use crate::goal_curation::GoalProgress;
    use crate::ooda_actions::dispatch_actions;
    use crate::ooda_actions::test_helpers::*;
    use crate::ooda_loop::{ActionKind, OodaState, PlannedAction};

    #[test]
    fn session_identity_describes_pm_architect_not_coder() {
        let (session, captured) = MockSession::new_ok("PROGRESS: 25", vec![]);
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::InProgress { percent: 20 }, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        let input = captured.borrow();
        let input = input.as_ref().expect("session should have received input");
        let id = &input.identity_context;

        // Must describe PM architect role, not a coder.
        assert!(
            id.contains("PM architect"),
            "identity should mention PM architect, got: {id}"
        );
        assert!(
            id.contains("amplihack") || id.contains("coding sessions"),
            "identity should mention managing coding sessions, got: {id}"
        );
        assert!(
            !id.to_lowercase().contains("you write code")
                && !id.to_lowercase().contains("you are a coder"),
            "identity must NOT describe Simard as a coder, got: {id}"
        );
    }

    #[test]
    fn session_objective_includes_assessment_steps() {
        let (session, captured) = MockSession::new_ok("PROGRESS: 30", vec![]);
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::InProgress { percent: 10 }, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        let input = captured.borrow();
        let input = input.as_ref().expect("session should have received input");
        let obj = &input.objective;

        // Objective must include the goal ID and description.
        assert!(obj.contains("g1"), "objective should contain goal ID");

        // Must instruct assessment of goal status.
        assert!(
            obj.to_lowercase().contains("assess") || obj.to_lowercase().contains("check"),
            "objective should instruct assessment, got: {obj}"
        );

        // Must mention creating GitHub issues for work.
        assert!(
            obj.to_lowercase().contains("github issue") || obj.to_lowercase().contains("issue"),
            "objective should mention creating issues, got: {obj}"
        );

        // Must mention launching amplihack sessions.
        assert!(
            obj.contains("simard engineer") || obj.contains("amplihack copilot"),
            "objective should mention delegation commands, got: {obj}"
        );

        // Must request a PROGRESS line in the response.
        assert!(
            obj.contains("PROGRESS"),
            "objective should request PROGRESS assessment, got: {obj}"
        );
    }

    #[test]
    fn session_progress_comes_from_agent_response_not_auto_bump() {
        // Agent reports PROGRESS: 55 — goal should become 55%, not current+10.
        let (session, _captured) = MockSession::new_ok(
            "Assessed the goal. Created issue #42.\nPROGRESS: 55",
            vec![],
        );
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::InProgress { percent: 20 }, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        assert!(outcomes[0].success);
        // Progress must be 55 (from agent response), NOT 30 (20+10 auto-bump).
        assert_eq!(
            state.active_goals.active[0].status,
            GoalProgress::InProgress { percent: 55 },
            "progress should come from agent's PROGRESS line, not auto-bump"
        );
    }

    #[test]
    fn session_no_progress_marker_preserves_current() {
        // Agent does NOT include a PROGRESS line — current progress must be preserved.
        let (session, _captured) = MockSession::new_ok(
            "Checked the repo. Everything looks fine.",
            vec!["no markers here".to_string()],
        );
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::InProgress { percent: 40 }, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        assert!(outcomes[0].success);
        // Must stay at 40%, NOT bumped to 50%.
        assert_eq!(
            state.active_goals.active[0].status,
            GoalProgress::InProgress { percent: 40 },
            "without PROGRESS marker, progress must be preserved (not auto-bumped)"
        );
    }

    #[test]
    fn session_progress_100_completes_goal() {
        let (session, _captured) = MockSession::new_ok("PROGRESS: 100", vec![]);
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::InProgress { percent: 80 }, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        assert!(outcomes[0].success);
        assert_eq!(state.active_goals.active[0].status, GoalProgress::Completed,);
    }

    #[test]
    fn session_run_turn_failure_returns_error_outcome() {
        let session = MockSession::new_err("connection lost");
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::InProgress { percent: 10 }, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        assert!(!outcomes[0].success);
        assert!(outcomes[0].detail.contains("session run_turn failed"));
        // Progress must NOT change on error.
        assert_eq!(
            state.active_goals.active[0].status,
            GoalProgress::InProgress { percent: 10 },
        );
    }

    #[test]
    fn session_objective_includes_environment_context() {
        let (session, captured) = MockSession::new_ok("PROGRESS: 20", vec![]);
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::NotStarted, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        let input = captured.borrow();
        let input = input.as_ref().expect("session should have received input");
        let obj = &input.objective;

        // Objective should include environment context (git status, issues, commits).
        assert!(
            obj.contains("Git status") || obj.contains("git status"),
            "objective should include environment context"
        );
    }

    #[test]
    fn session_not_started_goal_reports_0_percent_in_objective() {
        let (session, captured) = MockSession::new_ok("PROGRESS: 5", vec![]);
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::NotStarted, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        let input = captured.borrow();
        let input = input.as_ref().unwrap();
        // NotStarted should show 0% in the objective.
        assert!(
            input.objective.contains("0% complete"),
            "NotStarted goal should report 0% in objective"
        );
    }

    #[test]
    fn session_outcome_includes_verification_counts() {
        let (session, _) = MockSession::new_ok("Created an issue. PROGRESS: 20", vec![]);
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::NotStarted, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        let outcomes = dispatch_actions(&[action], &mut bridges, &mut state).unwrap();
        // The outcome detail should include verification counts.
        assert!(
            outcomes[0].detail.contains("verified="),
            "outcome should include verification counts, got: {}",
            outcomes[0].detail,
        );
    }

    #[test]
    fn objective_includes_concrete_commands() {
        let (session, captured) = MockSession::new_ok("PROGRESS: 10", vec![]);
        let mut bridges = bridges_with_session(session);
        let board = board_with_goal("g1", GoalProgress::NotStarted, None);
        let mut state = OodaState::new(board);
        let action = PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("g1".into()),
            description: "advance".into(),
        };
        dispatch_actions(&[action], &mut bridges, &mut state).unwrap();

        let input = captured.borrow();
        let input = input.as_ref().unwrap();
        assert!(
            input.objective.contains("gh issue create"),
            "objective should include concrete gh issue create command"
        );
        assert!(
            input.objective.contains("amplihack copilot"),
            "objective should include amplihack copilot command"
        );
        assert!(
            input.objective.contains("cargo test"),
            "objective should include cargo test command"
        );
    }
}
