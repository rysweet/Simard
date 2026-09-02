//! Session-based goal advancement — delegates work to a base-type agent.
//!
//! The orchestrator LLM emits **explicit contract prose only** (no JSON). Two
//! response shapes are supported:
//!
//! 1. `ACTION: SPAWN_ENGINEER` + `TASK:` block → dispatched as a
//!    `SpawnEngineer` task description.
//! 2. `NO ACTION` + `REASON:` → dispatched as a `NoAction` outcome.
//!
//! Both shapes optionally accept exactly one uppercase `PROGRESS: NN` marker
//! (0..=100) that updates the goal's recorded completion percentage.
//!
//! See `prompt_assets/simard/goal_session_objective.md` for the operator-
//! facing version of this contract.

#[cfg(test)]
use crate::ooda_loop::ActionOutcome;

/// The outcome of a single LLM-driven goal-advance turn.
///
/// Carries both the user-visible [`ActionOutcome`] and the parsed
/// [`GoalAction`] (when the LLM emitted a non-empty response), so the
/// upstream dispatcher in `advance_goal/mod.rs` can take side-effecting
/// follow-up steps such as actually spawning the engineer subprocess.
#[cfg(test)]
pub(crate) struct GoalSessionResult {
    pub(super) outcome: ActionOutcome,
    pub(super) action: Option<GoalAction>,
}

#[cfg(test)]
mod advance;
#[cfg(test)]
mod input;
#[cfg(test)]
mod outcome;

#[cfg(test)]
pub(crate) use advance::{
    advance_goal_with_session, apply_goal_advance_result, outcome_made_no_progress,
};
#[cfg(test)]
pub(crate) use input::build_goal_advance_input;
#[cfg(test)]
pub(crate) use outcome::GoalAction;
#[cfg(not(test))]
pub(crate) fn outcome_made_no_progress(outcome: &crate::ooda_loop::ActionOutcome) -> bool {
    outcome.success
        && outcome
            .detail
            .starts_with("typed no-action committed: outcome=")
}
#[cfg(test)]
use outcome::{
    GoalSessionParse, OrchestratorDecision, parse_orchestrator_response_strict,
    truncate_for_outcome,
};
#[cfg(test)]
use outcome::{OUTCOME_TEXT_MAX, parse_orchestrator_response};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_response_returns_none() {
        assert_eq!(parse_orchestrator_response(""), None);
        assert_eq!(parse_orchestrator_response("   "), None);
        assert_eq!(parse_orchestrator_response("\n\t  \r\n"), None);
    }

    #[test]
    fn free_form_prose_is_invalid_contract() {
        let response = "Run cargo test --lib prioritization and report which tests fail.";
        assert_eq!(parse_orchestrator_response(response), None);
    }

    #[test]
    fn explicit_spawn_marker_extracts_task_body() {
        let expected_task = "Run cargo test --lib prioritization and report which tests fail.";
        let response = format!("ACTION: SPAWN_ENGINEER\nTASK:\n{expected_task}\nPROGRESS: 60");
        let decision = parse_orchestrator_response(&response).expect("valid explicit spawn");
        assert_eq!(decision.progress_pct, Some(60));
        match decision.action {
            GoalAction::SpawnEngineer { task, files, issue } => {
                assert_eq!(task, expected_task);
                assert!(files.is_empty());
                assert!(issue.is_none());
            }
            other => panic!("expected SpawnEngineer, got {other:?}"),
        }
    }

    #[test]
    fn no_action_marker_requires_reason_and_extracts_reason() {
        let expected_reason =
            "Another subordinate (engineer-foo-1234) is already working this goal.";
        let response = format!("NO ACTION\nREASON: {expected_reason}");
        let decision = parse_orchestrator_response(&response).expect("valid no-action");
        match decision.action {
            GoalAction::NoAction { reason } => {
                assert_eq!(reason, expected_reason);
            }
            other => panic!("expected NoAction, got {other:?}"),
        }
    }

    #[test]
    fn no_action_without_reason_is_invalid_contract() {
        assert_eq!(parse_orchestrator_response("NO ACTION\nPROGRESS: 0"), None);
    }

    #[test]
    fn no_action_marker_inside_a_sentence_is_invalid_contract() {
        let response = "We should take no action against this issue until QA confirms.";
        assert_eq!(parse_orchestrator_response(response), None);
    }

    #[test]
    fn lowercase_or_underscore_no_action_markers_are_invalid_contract() {
        for marker in ["no action", "No Action", "NO_ACTION", "no_action"] {
            let response = format!("{marker}\nREASON: blocked on external review");
            assert_eq!(
                parse_orchestrator_response(&response),
                None,
                "marker '{marker}' must not be accepted outside the explicit contract"
            );
        }
    }

    #[test]
    fn missing_task_for_spawn_is_invalid_contract() {
        assert_eq!(
            parse_orchestrator_response("ACTION: SPAWN_ENGINEER\nPROGRESS: 20"),
            None
        );
    }

    #[test]
    fn progress_marker_extracted_from_no_action() {
        let response = "NO ACTION\nREASON: Waiting on PR review.\nPROGRESS: 80";
        let decision = parse_orchestrator_response(response).expect("valid no-action");
        assert_eq!(decision.progress_pct, Some(80));
        assert!(matches!(decision.action, GoalAction::NoAction { .. }));
    }

    #[test]
    fn progress_marker_above_100_is_invalid_contract() {
        let response = "NO ACTION\nREASON: nearly done.\nPROGRESS: 250";
        assert_eq!(parse_orchestrator_response(response), None);
    }

    #[test]
    fn lowercase_progress_marker_is_invalid_contract() {
        let response = "NO ACTION\nREASON: waiting.\nprogress:45";
        assert_eq!(parse_orchestrator_response(response), None);
    }

    #[test]
    fn unicode_line_before_progress_marker_does_not_panic() {
        let response = "ééééé status update\nNO ACTION\nREASON: waiting.\nPROGRESS: 45";
        let decision = parse_orchestrator_response(response).expect("valid no-action");
        assert_eq!(decision.progress_pct, Some(45));
    }

    #[test]
    fn progress_word_inside_token_does_not_match() {
        let response = "ACTION: SPAWN_ENGINEER\nTASK:\nBuild inprogress:waiting for tests";
        let decision = parse_orchestrator_response(response).expect("valid spawn");
        assert_eq!(decision.progress_pct, None);
    }

    #[test]
    fn duplicate_progress_markers_are_invalid_contract() {
        let response = "NO ACTION\nREASON: evidence is mixed.\nPROGRESS: 40\nPROGRESS: 80";
        assert_eq!(parse_orchestrator_response(response), None);
    }

    #[test]
    fn conflicting_action_markers_are_invalid_contract() {
        let response =
            "NO ACTION\nREASON: already running.\nACTION: SPAWN_ENGINEER\nTASK:\nStart another.";
        assert_eq!(parse_orchestrator_response(response), None);
    }

    #[test]
    fn unknown_action_marker_is_invalid_contract() {
        let response = "ACTION: MERGE_PR\nTASK:\nMerge PR #4042 directly.";
        assert_eq!(parse_orchestrator_response(response), None);
    }

    #[test]
    fn no_progress_marker_means_none() {
        let response = "ACTION: SPAWN_ENGINEER\nTASK:\nFix #1234.";
        let decision = parse_orchestrator_response(response).expect("valid spawn");
        assert_eq!(decision.progress_pct, None);
        match decision.action {
            GoalAction::SpawnEngineer { task, .. } => assert_eq!(task, "Fix #1234."),
            other => panic!("expected SpawnEngineer, got {other:?}"),
        }
    }

    #[test]
    fn truncate_handles_utf8_char_boundary() {
        // 256 bytes of ASCII + a multi-byte char — must not split the char.
        let s = format!("{}é", "x".repeat(OUTCOME_TEXT_MAX - 1));
        let truncated = truncate_for_outcome(&s);
        // The 'é' is 2 bytes; we should truncate at byte 254 to keep it whole
        // (or earlier), then append the ellipsis.
        assert!(truncated.ends_with('…'));
        // Must be valid UTF-8 (would have panicked on slice boundary otherwise).
        assert!(truncated.is_ascii() || truncated.chars().count() > 0);
    }
}
