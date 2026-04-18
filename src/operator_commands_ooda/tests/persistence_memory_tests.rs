//! Unit tests for `persist_cycle_to_memory` — the cognitive memory
//! persistence path that was previously uncovered.

use super::*;
use crate::ooda_loop::ActionKind;

#[test]
fn persist_cycle_to_memory_succeeds_for_minimal_report() {
    let bridges = crate::ooda_actions::test_helpers::test_bridges();
    let report = make_test_report(1);
    // Should not panic — best-effort persistence
    super::super::persistence::persist_cycle_to_memory(&bridges, &report);
}

#[test]
fn persist_cycle_to_memory_with_goals_and_outcomes() {
    let bridges = crate::ooda_actions::test_helpers::test_bridges();
    let report = make_report_with_goals_and_outcomes();
    super::super::persistence::persist_cycle_to_memory(&bridges, &report);
}

#[test]
fn persist_cycle_to_memory_with_zero_outcomes() {
    let bridges = crate::ooda_actions::test_helpers::test_bridges();
    let mut report = make_test_report(5);
    report.outcomes.clear();
    super::super::persistence::persist_cycle_to_memory(&bridges, &report);
}

#[test]
fn persist_cycle_to_memory_with_all_failed_outcomes() {
    use crate::ooda_loop::{ActionOutcome, PlannedAction};

    let bridges = crate::ooda_actions::test_helpers::test_bridges();
    let mut report = make_test_report(10);
    report.outcomes = vec![
        ActionOutcome {
            action: PlannedAction {
                kind: ActionKind::AdvanceGoal,
                goal_id: Some("g1".to_string()),
                description: "try advance".to_string(),
            },
            success: false,
            detail: "blocked".to_string(),
        },
        ActionOutcome {
            action: PlannedAction {
                kind: ActionKind::RunGymEval,
                goal_id: None,
                description: "eval".to_string(),
            },
            success: false,
            detail: "timeout".to_string(),
        },
    ];
    super::super::persistence::persist_cycle_to_memory(&bridges, &report);
}

#[test]
fn persist_cycle_report_and_memory_together() {
    let bridges = crate::ooda_actions::test_helpers::test_bridges();
    let dir = tempfile::tempdir().unwrap();
    let report = make_report_with_goals_and_outcomes();

    persist_cycle_report(dir.path(), &report);
    super::super::persistence::persist_cycle_to_memory(&bridges, &report);

    // File should exist from persist_cycle_report
    let path = dir.path().join("cycle_reports").join("cycle_7.json");
    assert!(path.exists());
}

#[test]
fn persist_cycle_to_memory_with_open_issues() {
    use crate::ooda_loop::{EnvironmentSnapshot, GoalSnapshot, Observation};

    let bridges = crate::ooda_actions::test_helpers::test_bridges();
    let mut report = make_test_report(3);
    report.observation = Observation {
        goal_statuses: vec![GoalSnapshot {
            id: "g1".to_string(),
            description: "fix bug".to_string(),
            progress: GoalProgress::InProgress { percent: 50 },
        }],
        gym_health: None,
        memory_stats: CognitiveStatistics::default(),
        pending_improvements: vec![],
        environment: EnvironmentSnapshot {
            git_status: String::new(),
            open_issues: vec!["issue-1".to_string(), "issue-2".to_string()],
            recent_commits: vec!["abc123".to_string()],
        },
    };
    super::super::persistence::persist_cycle_to_memory(&bridges, &report);
}
