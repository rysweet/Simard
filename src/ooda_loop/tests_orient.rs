use super::orient::*;
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
        eval_watchdog: None,
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
    let priorities = orient(&obs, &board, &std::collections::HashMap::new()).unwrap();
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
    let priorities = orient(&obs, &board, &std::collections::HashMap::new()).unwrap();
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
    let priorities = orient(&obs, &board, &std::collections::HashMap::new()).unwrap();
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
    let priorities = orient(&obs, &board, &std::collections::HashMap::new()).unwrap();
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

    let prio_with = orient(
        &make_observation(env_with_issue),
        &board,
        &std::collections::HashMap::new(),
    )
    .unwrap();
    let prio_without = orient(
        &make_observation(env_without),
        &board,
        &std::collections::HashMap::new(),
    )
    .unwrap();
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
    let prio_dirty = orient(
        &make_observation(env_dirty),
        &board,
        &std::collections::HashMap::new(),
    )
    .unwrap();
    let prio_clean = orient(
        &make_observation(env_clean),
        &board,
        &std::collections::HashMap::new(),
    )
    .unwrap();
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
        eval_watchdog: None,
    };
    let priorities = orient(&obs, &board, &std::collections::HashMap::new()).unwrap();
    assert!(
        priorities.iter().any(|p| p.goal_id == "__memory__"),
        "should add memory consolidation priority"
    );
}
