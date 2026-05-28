//! Goal-session "advance" + "no-action" outcome computation.
//!
//! The orchestrator LLM emits prose; this module parses that prose into a
//! [`GoalAction`] (via [`parse_orchestrator_response`]), then dispatches:
//!
//! * [`GoalAction::SpawnEngineer`] → upstream caller in
//!   `advance_goal/mod.rs` performs the actual subprocess spawn.
//! * [`GoalAction::NoAction`] → record an outcome without spawning, and
//!   apply any `PROGRESS: NN` marker to the goal board.
//!
//! Both branches honour an optional `PROGRESS: NN` marker.
//!
//! All progress-mutation paths route through
//! [`crate::goal_curation::update_goal_progress_with_evidence`] so a
//! `PROGRESS: NN` line that asks for an *increase* is rejected unless
//! the LLM-backed reviewer confirms it (issue #1967, #2007).

use chrono::Utc;

use crate::goal_curation::progress_evidence::EvidenceDecision;
use crate::goal_curation::{GoalBoard, GoalProgress, update_goal_progress_with_evidence};
use crate::ooda_loop::{ActionOutcome, OodaState, PlannedAction};

use super::super::make_outcome;
use super::{
    GoalAction, GoalSessionResult, OrchestratorDecision, parse_orchestrator_response,
    truncate_for_outcome,
};

/// Apply a `PROGRESS: NN` marker (or no marker) to the goal board and
/// return a successful [`ActionOutcome`] describing the no-op cycle.
///
/// Used by both the `NO ACTION` dispatch path and (for backwards-compat
/// callers / tests) directly. The function never spawns a subprocess.
pub(crate) fn assess_only_outcome(
    action: &PlannedAction,
    memory: &dyn crate::cognitive_memory::CognitiveMemoryOps,
    checker: &dyn crate::goal_curation::progress_evidence::ProgressEvidenceChecker,
    board: &mut GoalBoard,
    goal_id: &str,
    reason: &str,
    progress_pct: Option<u8>,
) -> ActionOutcome {
    let reason_short = truncate_for_outcome(reason);

    let Some(pct) = progress_pct else {
        // No PROGRESS marker — record the no-action outcome with the
        // current goal status untouched.
        eprintln!(
            "[simard] OODA goal-action no-action for '{}': {}",
            goal_id, reason_short,
        );
        let detail = format!("no-action: {} (goal '{}')", reason_short, goal_id);
        return make_outcome(action, true, detail);
    };

    let new_progress = if pct >= 100 {
        GoalProgress::Completed
    } else if pct == 0 {
        GoalProgress::NotStarted
    } else {
        GoalProgress::InProgress {
            percent: pct as u32,
        }
    };

    match update_goal_progress_with_evidence(
        board,
        goal_id,
        new_progress,
        checker,
        memory,
        Utc::now(),
    ) {
        Ok(EvidenceDecision::Accept { .. }) => {
            eprintln!(
                "[simard] OODA goal-action no-action for '{}': {} (progress={}%)",
                goal_id, reason_short, pct,
            );
            let detail = format!(
                "no-action: {} (progress={}%, goal '{}')",
                reason_short, pct, goal_id,
            );
            make_outcome(action, true, detail)
        }
        Ok(EvidenceDecision::Reject { reason: rej }) => {
            eprintln!(
                "[simard] OODA goal-action no-action REJECTED progress for '{}': {} (proposed={}%, reason={})",
                goal_id, reason_short, pct, rej,
            );
            let detail = format!(
                "no-action: progress claim rejected (reviewer): {rej} (goal '{}', proposed={}%)",
                goal_id, pct,
            );
            make_outcome(action, true, detail)
        }
        Err(e) => {
            eprintln!(
                "[simard] OODA goal-action no-action FAILED to update progress for '{}': {} (reason='{}', progress={}%)",
                goal_id, e, reason_short, pct,
            );
            let detail = format!(
                "no-action failed: update_goal_progress error for goal '{}': {} (reason='{}', progress={}%)",
                goal_id, e, reason_short, pct,
            );
            make_outcome(action, false, detail)
        }
    }
}

/// Advance a goal using a base-type session's `run_turn`.
///
/// Simard acts as a PM architect: she assesses the goal, decides whether to
/// delegate to an engineer subprocess, and tracks progress based on the
/// engineer's reported outcome — never by auto-incrementing.
pub(crate) fn advance_goal_with_session(
    action: &PlannedAction,
    memory: &dyn crate::cognitive_memory::CognitiveMemoryOps,
    checker: &dyn crate::goal_curation::progress_evidence::ProgressEvidenceChecker,
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

    // Load the objective instructions via prompt_store (runtime-overridable, embedded fallback).
    let goal_session_objective =
        crate::ooda_brain::prompt_store::global().load("goal_session_objective.md");

    // Build the objective in a single pre-sized buffer to avoid intermediate allocations.
    let mut objective = String::with_capacity(1024);
    let _ = write!(
        objective,
        "Goal '{}' ({}% complete): {}\n\n{}\n\nEnvironment context:\n- Git status: ",
        goal.id,
        percent,
        goal.description,
        goal_session_objective.trim(),
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
            let parsed = parse_orchestrator_response(&outcome.execution_summary);

            let Some(OrchestratorDecision {
                action: goal_action,
                progress_pct,
            }) = parsed
            else {
                // Truly empty response — nothing for the engineer to act
                // on. Visible failure.
                eprintln!(
                    "[simard] OODA goal-action EMPTY response for '{}': LLM returned no content",
                    goal.id,
                );
                let detail = format!(
                    "goal-action empty response for goal '{}': LLM returned no content",
                    goal.id,
                );
                return GoalSessionResult {
                    outcome: make_outcome(action, false, detail),
                    action: None,
                };
            };

            match goal_action {
                GoalAction::NoAction { ref reason } => {
                    let outcome = assess_only_outcome(
                        action,
                        memory,
                        checker,
                        &mut state.active_goals,
                        &goal.id,
                        reason,
                        progress_pct,
                    );
                    GoalSessionResult {
                        outcome,
                        action: Some(GoalAction::NoAction {
                            reason: reason.clone(),
                        }),
                    }
                }
                GoalAction::SpawnEngineer {
                    ref task,
                    ref files,
                    issue,
                } => {
                    // Apply the optional progress marker BEFORE spawning,
                    // so even if the engineer subprocess crashes the
                    // orchestrator's progress assessment is recorded.
                    //
                    // Routed through `update_goal_progress_with_evidence`
                    // (issue #1967): a pre-spawn bump that has no
                    // commits/PRs yet will be Rejected and the prior
                    // percent will be kept — by the time the engineer
                    // actually produces a commit, the next cycle will
                    // accept the same claim.
                    if let Some(pct) = progress_pct {
                        let new_progress = if pct >= 100 {
                            GoalProgress::Completed
                        } else if pct == 0 {
                            GoalProgress::NotStarted
                        } else {
                            GoalProgress::InProgress {
                                percent: pct as u32,
                            }
                        };
                        match update_goal_progress_with_evidence(
                            &mut state.active_goals,
                            &goal.id,
                            new_progress,
                            checker,
                            memory,
                            Utc::now(),
                        ) {
                            Ok(EvidenceDecision::Accept { .. }) => {}
                            Ok(EvidenceDecision::Reject { reason: rej }) => {
                                eprintln!(
                                    "[simard] OODA goal-action pre-spawn progress REJECTED for '{}': {} (proposed={}%)",
                                    goal.id, rej, pct,
                                );
                            }
                            Err(e) => {
                                eprintln!(
                                    "[simard] OODA goal-action progress update FAILED for '{}': {} (progress={}%)",
                                    goal.id, e, pct,
                                );
                            }
                        }
                    }

                    let truncated = truncate_for_outcome(task);
                    eprintln!(
                        "[simard] OODA goal-action: LLM emitted prose for '{}'; spawning engineer with prose as task: {}",
                        goal.id, truncated,
                    );
                    let detail = format!(
                        "spawn_engineer (from prose) for goal '{}': {}",
                        goal.id, truncated,
                    );
                    GoalSessionResult {
                        outcome: make_outcome(action, true, detail),
                        action: Some(GoalAction::SpawnEngineer {
                            task: task.clone(),
                            files: files.clone(),
                            issue,
                        }),
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
