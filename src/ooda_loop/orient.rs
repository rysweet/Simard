//! Orient phase: rank goals by urgency, informed by environment context.

use crate::error::SimardResult;
use crate::goal_curation::{GoalBoard, GoalProgress};

use super::{Observation, Priority};

/// Orient: rank goals by urgency, informed by environment context.
///
/// Base urgency: Blocked > not-started > in-progress > completed.
/// Environment signals (dirty working tree, open issues mentioning a goal)
/// can boost a goal's urgency so the OODA loop prioritises actionable work.
pub fn orient(observation: &Observation, goals: &GoalBoard) -> SimardResult<Vec<Priority>> {
    let env = &observation.environment;
    let has_dirty_tree = !env.git_status.is_empty();

    let mut priorities: Vec<Priority> = goals
        .active
        .iter()
        .map(|g| {
            let (mut urgency, mut reason) = match &g.status {
                GoalProgress::Blocked(r) => (1.0, format!("blocked: {r}")),
                GoalProgress::NotStarted => (0.8, "not yet started".to_string()),
                GoalProgress::InProgress { percent } => (
                    0.6 * (1.0 - (*percent as f64 / 100.0)),
                    format!("{percent}% complete"),
                ),
                GoalProgress::Completed => (0.0, "completed".to_string()),
            };

            // Boost urgency if an open issue mentions this goal.
            let mentioned_in_issues = env
                .open_issues
                .iter()
                .any(|title| title.to_lowercase().contains(&g.id.to_lowercase()));
            if mentioned_in_issues {
                urgency = (urgency + 0.1).min(1.0);
                reason = format!("{reason}; mentioned in open issue");
            }

            // Slight boost for in-progress goals when the tree is dirty
            // (indicates active development that may relate to this goal).
            if has_dirty_tree && matches!(g.status, GoalProgress::InProgress { .. }) {
                urgency = (urgency + 0.05).min(1.0);
                reason = format!("{reason}; dirty working tree");
            }

            Priority {
                goal_id: g.id.clone(),
                urgency,
                reason,
            }
        })
        .collect();

    if observation.memory_stats.episodic_count > 100 {
        priorities.push(Priority {
            goal_id: "__memory__".to_string(),
            urgency: 0.5,
            reason: format!(
                "episodic memory has {} entries, consolidation needed",
                observation.memory_stats.episodic_count
            ),
        });
    }

    if let Some(ref score) = observation.gym_health
        && score.overall < 0.7
    {
        priorities.push(Priority {
            goal_id: "__improvement__".to_string(),
            urgency: 0.7,
            reason: format!("gym overall {:.1}% below 70% target", score.overall * 100.0),
        });
    }

    priorities.sort_by(|a, b| {
        b.urgency
            .partial_cmp(&a.urgency)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(priorities)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress};
    use crate::gym_bridge::ScoreDimensions;
    use crate::gym_scoring::GymSuiteScore;
    use crate::memory_cognitive::CognitiveStatistics;
    use crate::ooda_loop::{EnvironmentSnapshot, Observation};

    fn make_gym_score(overall: f64, factual_accuracy: f64) -> GymSuiteScore {
        GymSuiteScore {
            suite_id: "progressive".to_string(),
            overall,
            dimensions: ScoreDimensions {
                factual_accuracy,
                specificity: 0.8,
                temporal_awareness: 0.7,
                source_attribution: 0.6,
                confidence_calibration: 0.5,
            },
            scenario_count: 5,
            scenarios_passed: 4,
            pass_rate: 0.8,
            recorded_at_unix_ms: None,
        }
    }

    fn make_board_with_goals(goals: Vec<ActiveGoal>) -> GoalBoard {
        let mut board = GoalBoard::new();
        board.active = goals;
        board
    }

    fn make_observation(env: EnvironmentSnapshot) -> Observation {
        Observation {
            goal_statuses: Vec::new(),
            gym_health: None,
            memory_stats: CognitiveStatistics::default(),
            pending_improvements: Vec::new(),
            environment: env,
        }
    }

    #[test]
    fn orient_blocked_goals_have_highest_urgency() {
        let goals = vec![
            ActiveGoal {
                id: "blocked".to_string(),
                description: "Blocked".to_string(),
                priority: 1,
                status: GoalProgress::Blocked("dependency".to_string()),
                assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            },
            ActiveGoal {
                id: "not-started".to_string(),
                description: "Not started".to_string(),
                priority: 1,
                status: GoalProgress::NotStarted,
                assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            },
        ];
        let board = make_board_with_goals(goals);
        let obs = make_observation(EnvironmentSnapshot::default());
        let priorities = orient(&obs, &board).unwrap();
        assert_eq!(priorities[0].goal_id, "blocked");
        assert!(priorities[0].urgency > priorities[1].urgency);
    }

    #[test]
    fn orient_completed_goals_have_zero_urgency() {
        let goals = vec![ActiveGoal {
            id: "done".to_string(),
            description: "Done".to_string(),
            priority: 1,
            status: GoalProgress::Completed,
            assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        }];
        let board = make_board_with_goals(goals);
        let obs = make_observation(EnvironmentSnapshot::default());
        let priorities = orient(&obs, &board).unwrap();
        assert!(
            priorities[0].urgency < f64::EPSILON,
            "completed goals should have zero urgency"
        );
    }

    #[test]
    fn orient_not_started_higher_urgency_than_in_progress() {
        let goals = vec![
            ActiveGoal {
                id: "new".to_string(),
                description: "New".to_string(),
                priority: 1,
                status: GoalProgress::NotStarted,
                assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            },
            ActiveGoal {
                id: "wip".to_string(),
                description: "WIP".to_string(),
                priority: 1,
                status: GoalProgress::InProgress { percent: 50 },
                assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            },
        ];
        let board = make_board_with_goals(goals);
        let obs = make_observation(EnvironmentSnapshot::default());
        let priorities = orient(&obs, &board).unwrap();
        let new_prio = priorities.iter().find(|p| p.goal_id == "new").unwrap();
        let wip_prio = priorities.iter().find(|p| p.goal_id == "wip").unwrap();
        assert!(new_prio.urgency > wip_prio.urgency);
    }

    #[test]
    fn orient_in_progress_urgency_decreases_with_percent() {
        let goals = vec![
            ActiveGoal {
                id: "early".to_string(),
                description: "Early".to_string(),
                priority: 1,
                status: GoalProgress::InProgress { percent: 10 },
                assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            },
            ActiveGoal {
                id: "late".to_string(),
                description: "Late".to_string(),
                priority: 1,
                status: GoalProgress::InProgress { percent: 90 },
                assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            },
        ];
        let board = make_board_with_goals(goals);
        let obs = make_observation(EnvironmentSnapshot::default());
        let priorities = orient(&obs, &board).unwrap();
        let early = priorities.iter().find(|p| p.goal_id == "early").unwrap();
        let late = priorities.iter().find(|p| p.goal_id == "late").unwrap();
        assert!(early.urgency > late.urgency);
    }

    #[test]
    fn orient_boosts_urgency_when_goal_mentioned_in_issues() {
        let goals = vec![ActiveGoal {
            id: "auth".to_string(),
            description: "Auth system".to_string(),
            priority: 1,
            status: GoalProgress::InProgress { percent: 50 },
            assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        }];
        let board = make_board_with_goals(goals.clone());
        let env_with_issue = EnvironmentSnapshot {
            git_status: String::new(),
            open_issues: vec!["Fix auth bug".to_string()],
            recent_commits: Vec::new(),
        };
        let env_without = EnvironmentSnapshot::default();

        let prio_with = orient(&make_observation(env_with_issue), &board).unwrap();
        let prio_without = orient(&make_observation(env_without), &board).unwrap();
        assert!(prio_with[0].urgency > prio_without[0].urgency);
        assert!(prio_with[0].reason.contains("open issue"));
    }

    #[test]
    fn orient_boosts_in_progress_when_dirty_tree() {
        let goals = vec![ActiveGoal {
            id: "wip".to_string(),
            description: "WIP".to_string(),
            priority: 1,
            status: GoalProgress::InProgress { percent: 50 },
            assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        }];
        let board = make_board_with_goals(goals);
        let env_dirty = EnvironmentSnapshot {
            git_status: "M src/main.rs".to_string(),
            open_issues: Vec::new(),
            recent_commits: Vec::new(),
        };
        let env_clean = EnvironmentSnapshot::default();
        let prio_dirty = orient(&make_observation(env_dirty), &board).unwrap();
        let prio_clean = orient(&make_observation(env_clean), &board).unwrap();
        assert!(prio_dirty[0].urgency > prio_clean[0].urgency);
        assert!(prio_dirty[0].reason.contains("dirty"));
    }

    #[test]
    fn orient_adds_memory_consolidation_when_episodic_exceeds_100() {
        let goals = vec![ActiveGoal {
            id: "g1".to_string(),
            description: "Goal".to_string(),
            priority: 1,
            status: GoalProgress::NotStarted,
            assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        }];
        let board = make_board_with_goals(goals);
        let obs = Observation {
            goal_statuses: Vec::new(),
            gym_health: None,
            memory_stats: CognitiveStatistics {
                episodic_count: 101,
                ..Default::default()
            },
            pending_improvements: Vec::new(),
            environment: EnvironmentSnapshot::default(),
        };
        let priorities = orient(&obs, &board).unwrap();
        assert!(
            priorities.iter().any(|p| p.goal_id == "__memory__"),
            "should add memory consolidation priority"
        );
    }

    #[test]
    fn orient_no_memory_consolidation_when_episodic_at_100() {
        let board = make_board_with_goals(vec![]);
        let obs = Observation {
            goal_statuses: Vec::new(),
            gym_health: None,
            memory_stats: CognitiveStatistics {
                episodic_count: 100,
                ..Default::default()
            },
            pending_improvements: Vec::new(),
            environment: EnvironmentSnapshot::default(),
        };
        let priorities = orient(&obs, &board).unwrap();
        assert!(
            !priorities.iter().any(|p| p.goal_id == "__memory__"),
            "should not add memory priority at exactly 100"
        );
    }

    #[test]
    fn orient_adds_improvement_priority_when_gym_below_70() {
        let board = make_board_with_goals(vec![]);
        let obs = Observation {
            goal_statuses: Vec::new(),
            gym_health: Some(make_gym_score(0.5, 0.6)),
            memory_stats: CognitiveStatistics::default(),
            pending_improvements: Vec::new(),
            environment: EnvironmentSnapshot::default(),
        };
        let priorities = orient(&obs, &board).unwrap();
        assert!(
            priorities.iter().any(|p| p.goal_id == "__improvement__"),
            "should add improvement priority when gym < 70%"
        );
    }

    #[test]
    fn orient_no_improvement_priority_when_gym_above_70() {
        let board = make_board_with_goals(vec![]);
        let obs = Observation {
            goal_statuses: Vec::new(),
            gym_health: Some(make_gym_score(0.8, 0.9)),
            memory_stats: CognitiveStatistics::default(),
            pending_improvements: Vec::new(),
            environment: EnvironmentSnapshot::default(),
        };
        let priorities = orient(&obs, &board).unwrap();
        assert!(
            !priorities.iter().any(|p| p.goal_id == "__improvement__"),
            "should not add improvement priority when gym >= 70%"
        );
    }

    #[test]
    fn orient_priorities_sorted_by_urgency_descending() {
        let goals = vec![
            ActiveGoal {
                id: "low".to_string(),
                description: "Low".to_string(),
                priority: 1,
                status: GoalProgress::Completed,
                assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            },
            ActiveGoal {
                id: "high".to_string(),
                description: "High".to_string(),
                priority: 1,
                status: GoalProgress::Blocked("x".to_string()),
                assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            },
            ActiveGoal {
                id: "mid".to_string(),
                description: "Mid".to_string(),
                priority: 1,
                status: GoalProgress::NotStarted,
                assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            },
        ];
        let board = make_board_with_goals(goals);
        let obs = make_observation(EnvironmentSnapshot::default());
        let priorities = orient(&obs, &board).unwrap();
        for i in 0..priorities.len() - 1 {
            assert!(priorities[i].urgency >= priorities[i + 1].urgency);
        }
    }
}
