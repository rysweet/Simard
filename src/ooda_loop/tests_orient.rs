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
        eval_watchdog: None,
    };
    let priorities = orient(&obs, &board, &std::collections::HashMap::new()).unwrap();
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
        eval_watchdog: None,
    };
    let priorities = orient(&obs, &board, &std::collections::HashMap::new()).unwrap();
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
        eval_watchdog: None,
    };
    let priorities = orient(&obs, &board, &std::collections::HashMap::new()).unwrap();
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
    let priorities = orient(&obs, &board, &std::collections::HashMap::new()).unwrap();
    for i in 0..priorities.len() - 1 {
        assert!(priorities[i].urgency >= priorities[i + 1].urgency);
    }
}

#[test]
fn orient_failure_cooldown_demotes_urgency() {
    let goals = vec![ActiveGoal {
        id: "broken-goal".to_string(),
        description: "always fails".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted, // base urgency 0.8
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    }];
    let board = make_board_with_goals(goals);
    let obs = make_observation(EnvironmentSnapshot::default());

    let mut counts = std::collections::HashMap::new();
    counts.insert("broken-goal".to_string(), 3); // 3 × 0.2 = 0.6 penalty

    let priorities = orient(&obs, &board, &counts).unwrap();
    assert_eq!(priorities.len(), 1);
    // 0.8 base − 0.6 penalty = 0.2
    assert!(
        (priorities[0].urgency - 0.2).abs() < 1e-9,
        "expected urgency ≈ 0.2 after 3 failures, got {}",
        priorities[0].urgency
    );
    assert!(priorities[0].reason.contains("3 consecutive failure"));
}

#[test]
fn orient_failure_cooldown_clamps_to_zero() {
    let goals = vec![ActiveGoal {
        id: "really-broken".to_string(),
        description: "many failures".to_string(),
        priority: 1,
        status: GoalProgress::Blocked("flaky".to_string()), // base urgency 1.0
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    }];
    let board = make_board_with_goals(goals);
    let obs = make_observation(EnvironmentSnapshot::default());

    let mut counts = std::collections::HashMap::new();
    counts.insert("really-broken".to_string(), 20); // huge penalty

    let priorities = orient(&obs, &board, &counts).unwrap();
    assert_eq!(priorities[0].urgency, 0.0);
}

#[test]
fn orient_no_demotion_when_failure_count_zero() {
    let goals = vec![ActiveGoal {
        id: "healthy-goal".to_string(),
        description: "OK".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    }];
    let board = make_board_with_goals(goals);
    let obs = make_observation(EnvironmentSnapshot::default());
    let counts = std::collections::HashMap::new();
    let priorities = orient(&obs, &board, &counts).unwrap();
    assert!((priorities[0].urgency - 0.8).abs() < 1e-9);
    assert!(!priorities[0].reason.contains("consecutive failure"));
}
