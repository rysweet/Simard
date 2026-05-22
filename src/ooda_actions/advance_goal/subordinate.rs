//! AdvanceGoal dispatch — routing, subordinate heartbeat, and session-based advancement.

use chrono::Utc;

use crate::agent_supervisor::{HeartbeatStatus, check_heartbeat};
use crate::goal_curation::progress_evidence::EvidenceDecision;
use crate::goal_curation::{
    GoalProgress, clear_goal_assignment, save_goal_board, update_goal_progress,
    update_goal_progress_with_evidence,
};
use crate::ooda_loop::{ActionOutcome, OodaBridges, OodaState, PlannedAction};

use crate::ooda_actions::make_outcome;

/// Advance a goal that has a subordinate assigned by checking heartbeat
/// and validating output artifacts.
pub fn advance_goal_with_subordinate(
    action: &PlannedAction,
    bridges: &mut OodaBridges,
    state: &mut OodaState,
    goal_id: &str,
    sub_name: &str,
) -> ActionOutcome {
    // Build a minimal handle for heartbeat checking. The worktree path is
    // taken from the OODA-owned EngineerWorktree (issue #1197) when
    // available so artifact validation looks at the engineer's own scope,
    // not the parent checkout. Falls back to "." for legacy/manual paths
    // that pre-date worktree isolation.
    let worktree_path = state
        .engineer_worktrees
        .get(goal_id)
        .map(|w| w.path().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let handle = crate::agent_supervisor::SubordinateHandle {
        pid: 0,
        agent_name: sub_name.to_string(),
        goal: goal_id.to_string(),
        worktree_path,
        spawn_time: 0,
        retry_count: 0,
        killed: false,
        session_name: String::new(),
    };

    match check_heartbeat(&handle, &*bridges.memory) {
        Ok(HeartbeatStatus::Alive { phase, .. }) => {
            // Check if subordinate reported completion with an outcome.
            if let Ok(Some(progress)) =
                crate::agent_goal_assignment::poll_progress(sub_name, &*bridges.memory)
                && progress.outcome.is_some()
            {
                // Subordinate claims completion — validate artifacts.
                return validate_subordinate_completion(
                    action,
                    &*bridges.progress_evidence,
                    &*bridges.memory,
                    state,
                    goal_id,
                    sub_name,
                    &progress,
                );
            }

            // Subordinate is alive and still working.
            //
            // Route the 50% heartbeat bump through
            // `update_goal_progress_with_evidence` (issue #1967): an
            // alive engineer is NOT evidence of progress. If the reviewer
            // cannot confirm meaningful progress since the last update,
            // the gate Rejects and the prior percent is preserved.
            let new_progress = GoalProgress::InProgress { percent: 50 };
            match update_goal_progress_with_evidence(
                &mut state.active_goals,
                goal_id,
                new_progress,
                &*bridges.progress_evidence,
                &*bridges.memory,
                Utc::now(),
            ) {
                Ok(EvidenceDecision::Accept { .. }) => {}
                Ok(EvidenceDecision::Reject { reason: rej }) => {
                    eprintln!(
                        "[simard] OODA subordinate heartbeat REJECTED 50% bump for \
                         goal '{goal_id}' (subordinate '{sub_name}' alive, no \
                         commits/PRs yet): {rej}"
                    );
                }
                Err(e) => {
                    eprintln!(
                        "[simard] OODA advance_goal FAILED to persist InProgress for \
                         goal '{goal_id}': {e}"
                    );
                    return make_outcome(
                        action,
                        false,
                        format!(
                            "subordinate '{sub_name}' alive (phase={phase}) but \
                             persisting InProgress for goal '{goal_id}' failed: {e}"
                        ),
                    );
                }
            }
            make_outcome(
                action,
                true,
                format!(
                    "subordinate '{sub_name}' alive (phase={phase}), goal '{goal_id}' in-progress"
                ),
            )
        }
        Ok(HeartbeatStatus::Stale { seconds_since }) => {
            // Subordinate is stale — check if it left behind any artifacts
            // before marking as failed.
            if let Ok(Some(progress)) =
                crate::agent_goal_assignment::poll_progress(sub_name, &*bridges.memory)
                && progress.outcome.is_some()
            {
                return validate_subordinate_completion(
                    action,
                    &*bridges.progress_evidence,
                    &*bridges.memory,
                    state,
                    goal_id,
                    sub_name,
                    &progress,
                );
            }

            eprintln!(
                "[simard] WARNING: subordinate '{sub_name}' stale ({seconds_since}s) \
                 with no completion outcome — clearing assignment so goal '{goal_id}' \
                 can be re-dispatched"
            );
            // Clear the assignment so dispatch_advance_goal re-enters the session
            // path and can spawn a fresh engineer on the next OODA cycle.
            if let Err(e) = clear_goal_assignment(&mut state.active_goals, goal_id) {
                eprintln!(
                    "[simard] OODA advance_goal FAILED to clear assignment for \
                     goal '{goal_id}': {e}"
                );
            } else if let Err(e) = save_goal_board(&state.active_goals, &*bridges.memory) {
                eprintln!(
                    "[simard] OODA advance_goal FAILED to persist goal board after \
                     clearing stale assignment for goal '{goal_id}': {e}"
                );
            }
            cleanup_engineer_worktree_for_goal(state, goal_id);
            make_outcome(
                action,
                false,
                format!(
                    "subordinate '{sub_name}' stale ({seconds_since}s) with no artifacts, \
                     goal '{goal_id}' assignment cleared for re-dispatch"
                ),
            )
        }
        Ok(HeartbeatStatus::Dead) => {
            // Subordinate is dead — check if it produced anything before dying.
            if let Ok(Some(progress)) =
                crate::agent_goal_assignment::poll_progress(sub_name, &*bridges.memory)
            {
                if progress.outcome.is_some() {
                    return validate_subordinate_completion(
                        action,
                        &*bridges.progress_evidence,
                        &*bridges.memory,
                        state,
                        goal_id,
                        sub_name,
                        &progress,
                    );
                }
                // Subordinate reported progress but no outcome — silent exit.
                eprintln!(
                    "[simard] WARNING: subordinate '{sub_name}' died without reporting \
                     an outcome — last phase='{}', last action='{}', \
                     exit_status={:?}, commits={}, prs={}",
                    progress.phase,
                    progress.last_action,
                    progress.exit_status,
                    progress.commits_produced,
                    progress.prs_produced,
                );
            } else {
                eprintln!(
                    "[simard] WARNING: subordinate '{sub_name}' is dead with no progress \
                     reports at all — it may have exited immediately without doing any work"
                );
            }

            if let Err(e) = clear_goal_assignment(&mut state.active_goals, goal_id) {
                eprintln!(
                    "[simard] OODA advance_goal FAILED to clear assignment for \
                     goal '{goal_id}': {e}"
                );
                cleanup_engineer_worktree_for_goal(state, goal_id);
                return make_outcome(
                    action,
                    false,
                    format!(
                        "subordinate '{sub_name}' exited with no artifacts and \
                         clearing assignment for goal '{goal_id}' failed: {e}"
                    ),
                );
            }
            if let Err(e) = save_goal_board(&state.active_goals, &*bridges.memory) {
                eprintln!(
                    "[simard] OODA advance_goal FAILED to persist goal board after \
                     clearing dead assignment for goal '{goal_id}': {e}"
                );
            }
            // Reap the per-engineer worktree (issue #1197).
            cleanup_engineer_worktree_for_goal(state, goal_id);
            make_outcome(
                action,
                false,
                format!(
                    "subordinate '{sub_name}' exited with no output artifacts, \
                     goal '{goal_id}' assignment cleared for re-dispatch"
                ),
            )
        }
        Err(e) => make_outcome(
            action,
            false,
            format!("heartbeat check failed for subordinate '{sub_name}': {e}"),
        ),
    }
}

/// Cleanup the per-goal engineer worktree owned by the OODA state.
///
/// Called from terminal paths (subordinate completed, dead, or stale-failed)
/// so the worktree dir + branch are reaped within one OODA cycle of the
/// engineer's exit. Idempotent — missing entries are silently a no-op.
fn cleanup_engineer_worktree_for_goal(state: &mut OodaState, goal_id: &str) {
    if let Some(worktree) = state.engineer_worktrees.remove(goal_id)
        && let Err(e) = worktree.cleanup()
    {
        tracing::warn!(
            target: "simard::engineer_worktree",
            goal = %goal_id,
            error = %e,
            "engineer worktree cleanup failed; Drop will run as a safety net",
        );
        // worktree drops here; if cleanup() already ran the swap guard
        // ensures Drop is a no-op.
    }
}

/// Validate that a subordinate's claimed completion produced real artifacts.
///
/// If the subordinate reports success but has zero commits and zero PRs,
/// the action is marked as failed so the OODA cycle can retry with a
/// different approach.
pub fn validate_subordinate_completion(
    action: &PlannedAction,
    checker: &dyn crate::goal_curation::progress_evidence::ProgressEvidenceChecker,
    memory: &dyn crate::cognitive_memory::CognitiveMemoryOps,
    state: &mut OodaState,
    goal_id: &str,
    sub_name: &str,
    progress: &crate::agent_goal_assignment::SubordinateProgress,
) -> ActionOutcome {
    let has_artifacts = progress.has_artifacts();
    let outcome_text = progress.outcome.as_deref().unwrap_or("unknown");

    if has_artifacts {
        // Route the Completed write through the progress-evidence gate
        // (issue #1967) for audit-trail consistency. Rule 1 (commit on
        // engineer branch) is satisfied by definition here because the
        // subordinate produced commits; the gate Accepts and stamps
        // `last_progress_update_at`.
        let new_progress = GoalProgress::Completed;
        match update_goal_progress_with_evidence(
            &mut state.active_goals,
            goal_id,
            new_progress,
            checker,
            memory,
            Utc::now(),
        ) {
            Ok(EvidenceDecision::Accept { .. }) => {}
            Ok(EvidenceDecision::Reject { reason: rej }) => {
                // Unexpected — subordinate produced artifacts so rule 1
                // should match. Log and fall through; the percent stays
                // where it was, but we still treat the action as
                // successful since the engineer did produce output.
                eprintln!(
                    "[simard] OODA validate_subordinate_completion: gate REJECTED \
                     Completed for goal '{goal_id}' despite artifacts: {rej}"
                );
            }
            Err(e) => {
                eprintln!(
                    "[simard] OODA advance_goal FAILED to persist Completed for \
                     goal '{goal_id}': {e}"
                );
                return make_outcome(
                    action,
                    false,
                    format!(
                        "subordinate '{sub_name}' produced {} commit(s) and {} PR(s) for \
                         goal '{goal_id}' but persisting Completed failed: {e}",
                        progress.commits_produced, progress.prs_produced,
                    ),
                );
            }
        }
        eprintln!(
            "[simard] subordinate '{sub_name}' completed goal '{goal_id}' — \
             {} commit(s), {} PR(s), outcome='{outcome_text}'",
            progress.commits_produced, progress.prs_produced,
        );
        // Reap the per-engineer worktree (issue #1197).
        cleanup_engineer_worktree_for_goal(state, goal_id);
        make_outcome(
            action,
            true,
            format!(
                "subordinate '{sub_name}' completed goal '{goal_id}' with \
                 {} commit(s) and {} PR(s)",
                progress.commits_produced, progress.prs_produced,
            ),
        )
    } else {
        // Subordinate claims success but produced nothing — this is the
        // silent exit bug (issue #905). Mark as failed for retry.
        //
        // `Blocked(reason)` is in the bypass set for the progress-evidence
        // gate (it does not increase the percent) so we keep the direct
        // `update_goal_progress` call here.
        eprintln!(
            "[simard] WARNING: subordinate '{sub_name}' reported outcome \
             '{outcome_text}' for goal '{goal_id}' but produced 0 commits \
             and 0 PRs — marking as failed for OODA retry"
        );
        if let Err(e) = update_goal_progress(
            &mut state.active_goals,
            goal_id,
            GoalProgress::Blocked(format!(
                "subordinate '{sub_name}' exited with outcome '{outcome_text}' \
                 but produced no commits or PRs"
            )),
        ) {
            eprintln!(
                "[simard] OODA advance_goal FAILED to persist Blocked for \
                 goal '{goal_id}': {e}"
            );
            cleanup_engineer_worktree_for_goal(state, goal_id);
            return make_outcome(
                action,
                false,
                format!(
                    "subordinate '{sub_name}' claimed '{outcome_text}' for goal '{goal_id}' \
                     but produced no artifacts and persisting Blocked failed: {e}"
                ),
            );
        }
        // Reap the per-engineer worktree (issue #1197).
        cleanup_engineer_worktree_for_goal(state, goal_id);
        make_outcome(
            action,
            false,
            format!(
                "subordinate '{sub_name}' claimed '{outcome_text}' for goal '{goal_id}' \
                 but produced 0 commits and 0 PRs — action failed, eligible for retry"
            ),
        )
    }
}
