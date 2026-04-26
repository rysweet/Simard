mod auto_reload_tests;
mod daemon_inline;
mod persistence_memory_tests;
mod persistence_tests;
mod report_tests;

use super::persistence::persist_cycle_report;
use crate::ooda_loop::{
    ActionKind, ActionOutcome, CycleReport, EnvironmentSnapshot, GoalSnapshot, Observation,
    PlannedAction, Priority,
};
use crate::{CognitiveStatistics, GoalProgress};

pub(crate) fn make_minimal_observation() -> Observation {
    Observation {
        goal_statuses: vec![],
        gym_health: None,
        memory_stats: CognitiveStatistics::default(),
        pending_improvements: vec![],
        environment: EnvironmentSnapshot::default(),
        eval_watchdog: None,
    }
}

pub(crate) fn make_test_report(cycle_number: u32) -> CycleReport {
    CycleReport {
        cycle_number,
        observation: make_minimal_observation(),
        priorities: vec![],
        planned_actions: vec![],
        outcomes: vec![],
    }
}

pub(crate) fn make_report_with_goals_and_outcomes() -> CycleReport {
    CycleReport {
        cycle_number: 7,
        observation: Observation {
            goal_statuses: vec![
                GoalSnapshot {
                    id: "goal-1".to_string(),
                    description: "First goal".to_string(),
                    progress: GoalProgress::InProgress { percent: 50 },
                },
                GoalSnapshot {
                    id: "goal-2".to_string(),
                    description: "Second goal".to_string(),
                    progress: GoalProgress::NotStarted,
                },
            ],
            gym_health: None,
            memory_stats: CognitiveStatistics::default(),
            pending_improvements: vec![],
            environment: EnvironmentSnapshot {
                git_status: "clean".to_string(),
                open_issues: vec!["issue-1".to_string()],
                recent_commits: vec![],
            },
            eval_watchdog: None,
        },
        priorities: vec![Priority {
            goal_id: "goal-1".to_string(),
            urgency: 0.8,
            reason: "High priority".to_string(),
        }],
        planned_actions: vec![PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some("goal-1".to_string()),
            description: "Work on goal 1".to_string(),
        }],
        outcomes: vec![
            ActionOutcome {
                action: PlannedAction {
                    kind: ActionKind::AdvanceGoal,
                    goal_id: Some("goal-1".to_string()),
                    description: "Work on goal 1".to_string(),
                },
                success: true,
                detail: "Completed".to_string(),
            },
            ActionOutcome {
                action: PlannedAction {
                    kind: ActionKind::RunGymEval,
                    goal_id: None,
                    description: "Run gym eval".to_string(),
                },
                success: false,
                detail: "Failed".to_string(),
            },
        ],
    }
}
