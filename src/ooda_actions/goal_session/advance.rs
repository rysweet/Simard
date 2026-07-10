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

    if let Some(goal) = board.active.iter_mut().find(|goal| goal.id == goal_id) {
        goal.current_activity = Some(format!("no-action evidence: {reason}"));
    }

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
///
/// Thin wrapper that builds the turn input, runs the (slow) LLM turn, and
/// applies the result. Concurrent dispatch (see `ooda_actions::concurrent`)
/// calls [`build_goal_advance_input`] and [`apply_goal_advance_result`]
/// directly so the slow `run_turn` happens with no lock held.
pub(crate) fn advance_goal_with_session(
    action: &PlannedAction,
    memory: &dyn crate::cognitive_memory::CognitiveMemoryOps,
    checker: &dyn crate::goal_curation::progress_evidence::ProgressEvidenceChecker,
    session: &mut dyn crate::base_types::BaseTypeSession,
    state: &mut OodaState,
    goal: &crate::goal_curation::ActiveGoal,
) -> GoalSessionResult {
    let observe_only =
        crate::read_only_guard::observe_only_enabled() || !state.identity_cognition.permits_spawn();
    let input =
        build_goal_advance_input(memory, state.prepared_context.as_ref(), goal, observe_only);
    let run_result = session.run_turn(input);
    apply_goal_advance_result(
        action,
        memory,
        checker,
        &mut state.active_goals,
        goal,
        run_result,
        observe_only,
    )
}

/// Build the goal-advance turn input from the goal, recalled context, and
/// fresh environment snapshot. Reinforces the surfaced memory context as a
/// side effect (issue #2395). Pure with respect to [`OodaState`] — takes the
/// recalled `prepared_context` by reference so the caller can hold the state
/// lock only briefly.
pub(crate) fn build_goal_advance_input(
    memory: &dyn crate::cognitive_memory::CognitiveMemoryOps,
    prepared_context: Option<&crate::memory_consolidation::PreparedContext>,
    goal: &crate::goal_curation::ActiveGoal,
    observe_only: bool,
) -> crate::base_types::BaseTypeTurnInput {
    use crate::base_types::BaseTypeTurnInput;
    use std::fmt::Write;

    let percent = match &goal.status {
        GoalProgress::InProgress { percent } => *percent,
        _ => 0,
    };

    // Gather fresh environment context so the agent sees current state.
    let env = crate::ooda_loop::gather_environment();

    // Load the objective instructions from the prompt store (runtime-overridable,
    // falls back to the compiled-in embed when no disk file exists).
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

    if observe_only {
        objective.push_str(
            "\n\n## Read-only observer contract\n\
             This identity is running with SIMARD_OBSERVE_ONLY=1. Do not ask for, \
             plan, or dispatch an engineer. Perform the allowed read-only inspection \
             in this session using only read commands, then respond with a line \
             containing exactly NO ACTION, a conservative PROGRESS: NN marker when \
             you have concrete evidence, and EVIDENCE/PROPOSALS bullets. If you \
             cannot gather new evidence, respond NO ACTION with PROGRESS: 0 and \
             state why.",
        );
    }

    // Append recalled memory context (facts, prospectives, procedures) when available.
    if let Some(ctx) = prepared_context {
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
        // PR-C (issue #2281, problem 4): inject Prior episodes
        // section. Omitted entirely when empty to avoid empty-section
        // noise. Each line includes the source label, the
        // monotonically-increasing temporal index, and content
        // truncated to 200 characters with an ellipsis.
        if !ctx.episodic_recall.is_empty() {
            objective.push_str("\n\n## Prior episodes (ranked by relevance)");
            for ep in &ctx.episodic_recall {
                let content = if ep.content.chars().count() > 200 {
                    let truncated: String = ep.content.chars().take(200).collect();
                    format!("{truncated}…")
                } else {
                    ep.content.clone()
                };
                let _ = write!(
                    objective,
                    "\n- [{}] [t={}] {}",
                    ep.source_label, ep.temporal_index, content
                );
            }
        }

        // Issue #2395: reinforce-on-use. The recalled facts / procedures /
        // episodes above were just surfaced into this cycle's prompt, so bump
        // their usage/recency now. Preparation recall is a pure read (so the
        // per-cycle recalls don't skew each other); this is the single point
        // where the reinforcement signal the ranked recall feeds on is written.
        crate::memory_consolidation::reinforce_prepared_context(memory, ctx);
    }

    const GOAL_SESSION_IDENTITY: &str =
        include_str!("../../../prompt_assets/simard/goal_session_identity.md");
    let identity_context = GOAL_SESSION_IDENTITY.trim().to_string();

    BaseTypeTurnInput {
        objective,
        identity_context,
        prompt_preamble: String::new(),
    }
}

/// Apply the result of a goal-advance `run_turn` to the goal board.
///
/// Splits the post-turn logic out of [`advance_goal_with_session`] so
/// concurrent dispatch can run the slow `run_turn` with no lock held, then
/// take a short lock to apply the parsed decision here. Mutates only the
/// supplied `board` (never the whole [`OodaState`]).
pub(crate) fn apply_goal_advance_result(
    action: &PlannedAction,
    memory: &dyn crate::cognitive_memory::CognitiveMemoryOps,
    checker: &dyn crate::goal_curation::progress_evidence::ProgressEvidenceChecker,
    board: &mut GoalBoard,
    goal: &crate::goal_curation::ActiveGoal,
    run_result: crate::error::SimardResult<crate::base_types::BaseTypeOutcome>,
    observe_only: bool,
) -> GoalSessionResult {
    match run_result {
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
                        board,
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
                    if observe_only {
                        let reason = format!(
                            "observe-only posture converted spawn request to no-action: {task}"
                        );
                        let outcome = assess_only_outcome(
                            action,
                            memory,
                            checker,
                            board,
                            &goal.id,
                            &reason,
                            progress_pct,
                        );
                        return GoalSessionResult {
                            outcome,
                            action: Some(GoalAction::NoAction { reason }),
                        };
                    }

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
                            board,
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

/// Classify an [`ActionOutcome`] as a *no shippable progress* cycle — the signal
/// the Fix-3 no-progress breaker counts (see
/// [`crate::goal_curation::no_progress_breaker`]).
///
/// A goal-advance cycle makes **no shippable progress** when the orchestrator
/// resolved to `NO ACTION` and either recorded no progress marker at all
/// ("I'll verify concretely…") **or** claimed a progress bump the reviewer
/// **rejected**. It makes *real* progress when it spawned an engineer or when
/// the reviewer **accepted** a progress advance.
///
/// This is the classifier half of the detail contract authored by
/// [`assess_only_outcome`] in this same module — kept co-located so the two stay
/// in lockstep (pinned by the tests below). The three success-`true` no-action
/// details are:
///
/// - pure no-action: `"no-action: {reason} (goal '{id}')"`               → no progress
/// - accepted bump:  `"no-action: {reason} (progress={pct}%, goal '{id}')"` → progress
/// - rejected bump:  `"no-action: progress claim rejected (reviewer): … (goal '{id}', proposed={pct}%)"` → no progress
///
/// The accepted-bump branch is the only one that emits the deterministic
/// `"(progress="` token, so the predicate keys on that token rather than the
/// free-form reviewer/orchestrator prose. A `success=false` outcome (empty
/// response, run error, or a failed progress update) is *not* a no-progress
/// no-op — it is already counted by the brain-failure safeguard's
/// `goal_failure_counts`, so this predicate excludes it to avoid double-counting.
pub(crate) fn outcome_made_no_progress(outcome: &ActionOutcome) -> bool {
    outcome.success
        && outcome.action.goal_id.is_some()
        && outcome.detail.starts_with("no-action:")
        && !outcome.detail.contains("(progress=")
}

#[cfg(test)]
mod tests_no_progress_classifier {
    use super::*;
    use crate::goal_curation::progress_evidence::{
        EvidenceDecision, NoopProgressEvidenceChecker, ProgressEvidenceChecker,
    };
    use crate::goal_curation::{ActiveGoal, GoalBoard};
    use crate::ooda_loop::PlannedAction;

    /// A progress-evidence checker that rejects every proposed increase — models
    /// the reviewer refuting an LLM progress claim.
    struct RejectAllChecker;
    impl ProgressEvidenceChecker for RejectAllChecker {
        fn check(
            &self,
            _goal: &ActiveGoal,
            _current: u32,
            _proposed: u32,
            _now: chrono::DateTime<Utc>,
        ) -> EvidenceDecision {
            EvidenceDecision::Reject {
                reason: "no commits or PR to substantiate the claimed progress".to_string(),
            }
        }
    }

    fn advance_goal_action(goal_id: &str) -> PlannedAction {
        PlannedAction {
            kind: crate::ooda_loop::ActionKind::AdvanceGoal,
            goal_id: Some(goal_id.to_string()),
            description: "advance".to_string(),
        }
    }

    fn board_with(goal_id: &str) -> GoalBoard {
        let mut board = GoalBoard::new();
        board
            .active
            .push(ActiveGoal::new(goal_id, "harden the supply chain", 1));
        board
    }

    fn mem() -> Box<dyn crate::cognitive_memory::CognitiveMemoryOps> {
        crate::ooda_actions::test_helpers::mock_memory()
    }

    // The classifier and `assess_only_outcome` are pinned together: these drive
    // the REAL author so a change to either detail format or predicate fails CI.

    #[test]
    fn pure_no_action_is_no_progress() {
        let action = advance_goal_action("g");
        let mut board = board_with("g");
        let outcome = assess_only_outcome(
            &action,
            &*mem(),
            &NoopProgressEvidenceChecker,
            &mut board,
            "g",
            "I'll verify concretely next cycle",
            None,
        );
        assert!(outcome.success);
        assert!(
            outcome_made_no_progress(&outcome),
            "pure no-action must count as no progress: {}",
            outcome.detail
        );
    }

    #[test]
    fn accepted_progress_bump_is_progress_not_no_progress() {
        let action = advance_goal_action("g");
        let mut board = board_with("g");
        let outcome = assess_only_outcome(
            &action,
            &*mem(),
            &NoopProgressEvidenceChecker, // accepts the bump
            &mut board,
            "g",
            "made real headway",
            Some(40),
        );
        assert!(outcome.success);
        assert!(
            !outcome_made_no_progress(&outcome),
            "an accepted progress advance must NOT count as no progress: {}",
            outcome.detail
        );
    }

    #[test]
    fn rejected_progress_bump_is_no_progress() {
        let action = advance_goal_action("g");
        let mut board = board_with("g");
        let outcome = assess_only_outcome(
            &action,
            &*mem(),
            &RejectAllChecker, // reviewer refutes the claim
            &mut board,
            "g",
            "claiming 60% with no evidence",
            Some(60),
        );
        assert!(outcome.success);
        assert!(
            outcome_made_no_progress(&outcome),
            "a rejected progress claim must count as no progress: {}",
            outcome.detail
        );
    }

    #[test]
    fn spawn_engineer_outcome_is_progress() {
        // A spawn-engineer detail does not start with "no-action:".
        let action = advance_goal_action("g");
        let outcome = make_outcome(
            &action,
            true,
            "spawn_engineer (from prose) for goal 'g': do the work".to_string(),
        );
        assert!(!outcome_made_no_progress(&outcome));
    }

    #[test]
    fn failed_no_action_update_is_not_counted() {
        // success=false is owned by the brain-failure safeguard, not this one.
        let action = advance_goal_action("g");
        let outcome = make_outcome(
            &action,
            false,
            "no-action failed: update_goal_progress error for goal 'g': boom".to_string(),
        );
        assert!(!outcome_made_no_progress(&outcome));
    }

    #[test]
    fn outcome_without_goal_id_is_not_counted() {
        let action = PlannedAction {
            kind: crate::ooda_loop::ActionKind::SafeUpdate,
            goal_id: None,
            description: "update".to_string(),
        };
        let outcome = make_outcome(&action, true, "no-action: whatever".to_string());
        assert!(!outcome_made_no_progress(&outcome));
    }
}
