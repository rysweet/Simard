use super::{make_minimal_observation, make_report_with_goals_and_outcomes, make_test_report};
use crate::ooda_loop::{EnvironmentSnapshot, OodaConfig, OodaState};
use crate::{CognitiveStatistics, GoalProgress};

// --- OodaConfig defaults ---

#[test]
fn ooda_config_default_values() {
    let config = OodaConfig::default();
    assert_eq!(config.max_concurrent_actions, 3);
    assert!(
        (config.improvement_threshold - 0.02).abs() < f64::EPSILON,
        "improvement_threshold should be 0.02"
    );
    assert_eq!(config.gym_suite_id, "progressive");
}

// --- summarize_cycle_report ---

#[test]
fn summarize_empty_report() {
    let report = make_test_report(1);
    let summary = crate::ooda_loop::summarize_cycle_report(&report);
    assert!(
        summary.contains("#1"),
        "summary should contain cycle number: {summary}"
    );
}

#[test]
fn summarize_report_with_outcomes() {
    let report = make_report_with_goals_and_outcomes();
    let summary = crate::ooda_loop::summarize_cycle_report(&report);
    assert!(
        summary.contains("#7"),
        "summary should contain cycle number: {summary}"
    );
    assert!(
        summary.contains("1/2"),
        "summary should contain success ratio: {summary}"
    );
}

#[test]
fn summarize_report_mentions_goals() {
    let report = make_report_with_goals_and_outcomes();
    let summary = crate::ooda_loop::summarize_cycle_report(&report);
    assert!(
        summary.contains("goals=2"),
        "summary should mention goal count: {summary}"
    );
}

#[test]
fn summarize_report_mentions_issues() {
    let report = make_report_with_goals_and_outcomes();
    let summary = crate::ooda_loop::summarize_cycle_report(&report);
    assert!(
        summary.contains("issues=1"),
        "summary should mention issue count: {summary}"
    );
}

// --- OodaState / OodaConfig ---

#[test]
fn ooda_state_new_has_zero_cycle_count() {
    let board = crate::goal_curation::GoalBoard::default();
    let state = OodaState::new(board);
    assert_eq!(state.cycle_count, 0);
}

// --- make_minimal_observation ---

#[test]
fn minimal_observation_has_empty_goals() {
    let obs = make_minimal_observation();
    assert!(obs.goal_statuses.is_empty());
    assert!(obs.pending_improvements.is_empty());
}

// --- report_with_goals_and_outcomes ---

#[test]
fn report_with_goals_has_two_goals() {
    let report = make_report_with_goals_and_outcomes();
    assert_eq!(report.observation.goal_statuses.len(), 2);
}

#[test]
fn report_with_goals_has_two_outcomes() {
    let report = make_report_with_goals_and_outcomes();
    assert_eq!(report.outcomes.len(), 2);
}

#[test]
fn report_with_goals_has_one_priority() {
    let report = make_report_with_goals_and_outcomes();
    assert_eq!(report.priorities.len(), 1);
    assert!((report.priorities[0].urgency - 0.8).abs() < f64::EPSILON);
}

#[test]
fn report_with_goals_has_one_planned_action() {
    let report = make_report_with_goals_and_outcomes();
    assert_eq!(report.planned_actions.len(), 1);
}

// --- summarize_cycle_report edge cases ---

#[test]
fn summarize_cycle_report_cycle_0() {
    let report = make_test_report(0);
    let summary = crate::ooda_loop::summarize_cycle_report(&report);
    assert!(summary.contains("#0"), "should handle cycle 0: {summary}");
}

#[test]
fn summarize_report_all_outcomes_succeed() {
    let mut report = make_report_with_goals_and_outcomes();
    for outcome in &mut report.outcomes {
        outcome.success = true;
    }
    let summary = crate::ooda_loop::summarize_cycle_report(&report);
    assert!(summary.contains("2/2"), "all should pass: {summary}");
}

#[test]
fn summarize_report_all_outcomes_fail() {
    let mut report = make_report_with_goals_and_outcomes();
    for outcome in &mut report.outcomes {
        outcome.success = false;
    }
    let summary = crate::ooda_loop::summarize_cycle_report(&report);
    assert!(summary.contains("0/2"), "none should pass: {summary}");
}

// --- EnvironmentSnapshot::default ---

#[test]
fn environment_snapshot_default_is_empty() {
    let env = EnvironmentSnapshot::default();
    assert!(env.open_issues.is_empty());
    assert!(env.recent_commits.is_empty());
}

// --- OodaState ---

#[test]
fn ooda_state_has_empty_active_goals() {
    let board = crate::goal_curation::GoalBoard::default();
    let state = OodaState::new(board);
    assert_eq!(state.cycle_count, 0);
    assert!(state.active_goals.active.is_empty());
}

// --- OodaConfig ---

#[test]
fn ooda_config_gym_suite_id_is_progressive() {
    let config = OodaConfig::default();
    assert_eq!(config.gym_suite_id, "progressive");
}

#[test]
fn ooda_config_max_concurrent_is_three() {
    let config = OodaConfig::default();
    assert_eq!(config.max_concurrent_actions, 3);
}

// --- report field accessors ---

#[test]
fn report_with_goals_outcome_detail_strings() {
    let report = make_report_with_goals_and_outcomes();
    assert_eq!(report.outcomes[0].detail, "Completed");
    assert_eq!(report.outcomes[1].detail, "Failed");
}

#[test]
fn report_with_goals_action_kinds() {
    use crate::ooda_loop::ActionKind;
    let report = make_report_with_goals_and_outcomes();
    assert!(matches!(
        report.outcomes[0].action.kind,
        ActionKind::AdvanceGoal
    ));
    assert!(matches!(
        report.outcomes[1].action.kind,
        ActionKind::RunGymEval
    ));
}

#[test]
fn report_with_goals_environment_has_git_status() {
    let report = make_report_with_goals_and_outcomes();
    assert_eq!(report.observation.environment.git_status, "clean");
}

#[test]
fn report_with_goals_priority_reason() {
    let report = make_report_with_goals_and_outcomes();
    assert_eq!(report.priorities[0].reason, "High priority");
    assert_eq!(report.priorities[0].goal_id, "goal-1");
}

#[test]
fn report_goal_progress_variants() {
    let report = make_report_with_goals_and_outcomes();
    assert!(matches!(
        report.observation.goal_statuses[0].progress,
        GoalProgress::InProgress { percent: 50 }
    ));
    assert!(matches!(
        report.observation.goal_statuses[1].progress,
        GoalProgress::NotStarted
    ));
}

// --- CognitiveStatistics default ---

#[test]
fn cognitive_statistics_default_all_zero() {
    let stats = CognitiveStatistics::default();
    assert_eq!(stats.total(), 0);
}

// --- summarize edge cases ---

#[test]
fn summarize_large_cycle_number() {
    let report = make_test_report(1_000_000);
    let summary = crate::ooda_loop::summarize_cycle_report(&report);
    assert!(
        summary.contains("1000000"),
        "should contain large number: {summary}"
    );
}
