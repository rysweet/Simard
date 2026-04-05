//! Cycle report summarization for logging and persistence.

use super::CycleReport;

/// Summarize a cycle report for logging/persistence.
pub fn summarize_cycle_report(report: &CycleReport) -> String {
    let succeeded = report.outcomes.iter().filter(|o| o.success).count();
    let total = report.outcomes.len();
    let env = &report.observation.environment;
    let dirty = if env.git_status.is_empty() {
        "clean"
    } else {
        "dirty"
    };
    format!(
        "OODA cycle #{}: {} priorities, {} actions ({}/{} succeeded), goals={}, issues={}, tree={}",
        report.cycle_number,
        report.priorities.len(),
        total,
        succeeded,
        total,
        report.observation.goal_statuses.len(),
        env.open_issues.len(),
        dirty,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_cognitive::CognitiveStatistics;
    use crate::ooda_loop::{
        ActionKind, ActionOutcome, CycleReport, EnvironmentSnapshot, GoalSnapshot, Observation,
        PlannedAction, Priority,
    };
    use crate::goal_curation::GoalProgress;

    #[test]
    fn summarize_cycle_report_format() {
        let report = CycleReport {
            cycle_number: 1,
            observation: Observation {
                goal_statuses: vec![GoalSnapshot {
                    id: "g1".to_string(),
                    description: "Goal 1".to_string(),
                    progress: GoalProgress::NotStarted,
                }],
                gym_health: None,
                memory_stats: CognitiveStatistics::default(),
                pending_improvements: Vec::new(),
                environment: EnvironmentSnapshot::default(),
            },
            priorities: vec![Priority {
                goal_id: "g1".to_string(),
                urgency: 0.8,
                reason: "not started".to_string(),
            }],
            planned_actions: vec![PlannedAction {
                kind: ActionKind::AdvanceGoal,
                goal_id: Some("g1".to_string()),
                description: "advance g1".to_string(),
            }],
            outcomes: vec![ActionOutcome {
                action: PlannedAction {
                    kind: ActionKind::AdvanceGoal,
                    goal_id: Some("g1".to_string()),
                    description: "advance g1".to_string(),
                },
                success: true,
                detail: "done".to_string(),
            }],
        };
        let summary = summarize_cycle_report(&report);
        assert!(summary.contains("cycle #1"));
        assert!(summary.contains("1 priorities"));
        assert!(summary.contains("1/1 succeeded"));
        assert!(summary.contains("goals=1"));
        assert!(summary.contains("tree=clean"));
    }

    #[test]
    fn summarize_cycle_report_dirty_tree() {
        let report = CycleReport {
            cycle_number: 2,
            observation: Observation {
                goal_statuses: Vec::new(),
                gym_health: None,
                memory_stats: CognitiveStatistics::default(),
                pending_improvements: Vec::new(),
                environment: EnvironmentSnapshot {
                    git_status: "M file.rs".to_string(),
                    open_issues: vec!["issue 1".to_string()],
                    recent_commits: Vec::new(),
                },
            },
            priorities: Vec::new(),
            planned_actions: Vec::new(),
            outcomes: Vec::new(),
        };
        let summary = summarize_cycle_report(&report);
        assert!(summary.contains("tree=dirty"));
        assert!(summary.contains("issues=1"));
    }

    #[test]
    fn summarize_cycle_report_mixed_outcomes() {
        let report = CycleReport {
            cycle_number: 3,
            observation: Observation {
                goal_statuses: Vec::new(),
                gym_health: None,
                memory_stats: CognitiveStatistics::default(),
                pending_improvements: Vec::new(),
                environment: EnvironmentSnapshot::default(),
            },
            priorities: Vec::new(),
            planned_actions: Vec::new(),
            outcomes: vec![
                ActionOutcome {
                    action: PlannedAction {
                        kind: ActionKind::AdvanceGoal,
                        goal_id: None,
                        description: "a".to_string(),
                    },
                    success: true,
                    detail: "ok".to_string(),
                },
                ActionOutcome {
                    action: PlannedAction {
                        kind: ActionKind::RunImprovement,
                        goal_id: None,
                        description: "b".to_string(),
                    },
                    success: false,
                    detail: "fail".to_string(),
                },
            ],
        };
        let summary = summarize_cycle_report(&report);
        assert!(summary.contains("1/2 succeeded"));
    }
}
