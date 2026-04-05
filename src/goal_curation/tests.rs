use super::*;
use crate::bridge_subprocess::InMemoryBridgeTransport;
use crate::memory_bridge::CognitiveMemoryBridge;
use serde_json::json;

fn mock_bridge() -> CognitiveMemoryBridge {
    let transport = InMemoryBridgeTransport::new("test-goals", |method, _params| match method {
        "memory.search_facts" => Ok(json!({"facts": []})),
        "memory.store_fact" => Ok(json!({"id": "sem_g1"})),
        "memory.store_episode" => Ok(json!({"id": "epi_g1"})),
        _ => Err(crate::bridge::BridgeErrorPayload {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    });
    CognitiveMemoryBridge::new(Box::new(transport))
}

fn sample_goal(id: &str, priority: u32) -> ActiveGoal {
    ActiveGoal {
        id: id.to_string(),
        description: format!("Goal {id}"),
        priority,
        status: GoalProgress::NotStarted,
        assigned_to: None,
    }
}

#[test]
fn enforce_max_active_goals() {
    let mut board = GoalBoard::new();
    for i in 1..=MAX_ACTIVE_GOALS {
        add_active_goal(&mut board, sample_goal(&format!("g{i}"), i as u32)).unwrap();
    }
    let err = add_active_goal(&mut board, sample_goal("g-overflow", 1)).unwrap_err();
    assert!(err.to_string().contains("capacity"));
}

#[test]
fn promote_backlog_to_active() {
    let mut board = GoalBoard::new();
    add_backlog_item(
        &mut board,
        BacklogItem {
            id: "bl-1".to_string(),
            description: "Research topic X".to_string(),
            source: "meeting".to_string(),
            score: 0.8,
        },
    )
    .unwrap();
    promote_to_active(&mut board, "bl-1", 2, Some("alice".to_string())).unwrap();
    assert_eq!(board.active.len(), 1);
    assert!(board.backlog.is_empty());
    assert_eq!(board.active[0].assigned_to.as_deref(), Some("alice"));
}

#[test]
fn update_progress_and_archive() {
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, sample_goal("g1", 1)).unwrap();
    update_goal_progress(&mut board, "g1", GoalProgress::Completed).unwrap();
    let archived = archive_completed(&mut board);
    assert_eq!(archived.len(), 1);
    assert!(board.active.is_empty());
}

#[test]
fn load_empty_board_from_bridge() {
    let bridge = mock_bridge();
    let board = load_goal_board(&bridge).unwrap();
    assert!(board.active.is_empty());
    assert!(board.backlog.is_empty());
}

#[test]
fn rejects_zero_priority() {
    let mut board = GoalBoard::new();
    let err = add_active_goal(
        &mut board,
        ActiveGoal {
            id: "bad".to_string(),
            description: "Zero priority".to_string(),
            priority: 0,
            status: GoalProgress::NotStarted,
            assigned_to: None,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("priority"));
}

#[test]
fn rejects_progress_over_100() {
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, sample_goal("g1", 1)).unwrap();
    let err = update_goal_progress(&mut board, "g1", GoalProgress::InProgress { percent: 200 })
        .unwrap_err();
    assert!(err.to_string().contains("100"));
}

#[test]
fn seed_default_board_adds_five_goals_to_empty_board() {
    let mut board = GoalBoard::new();
    let count = seed_default_board(&mut board);
    assert_eq!(count, 5);
    assert_eq!(board.active.len(), 5);
    for goal in &board.active {
        assert!(matches!(goal.status, GoalProgress::NotStarted));
    }
}

#[test]
fn seed_default_board_noop_when_board_has_goals() {
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, sample_goal("existing", 1)).unwrap();
    let count = seed_default_board(&mut board);
    assert_eq!(count, 0);
    assert_eq!(board.active.len(), 1);
}

#[test]
fn seed_default_board_is_idempotent() {
    let mut board = GoalBoard::new();
    seed_default_board(&mut board);
    let count = seed_default_board(&mut board);
    assert_eq!(count, 0);
    assert_eq!(board.active.len(), 5);
}
