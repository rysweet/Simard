//! Tests for the goal CRUD handlers in `goals.rs` (issue #1750).
//!
//! Each test uses [`HermeticState`] to isolate cognitive-memory state and
//! calls the async handler functions directly with constructed axum
//! extractor wrappers.

use axum::Json;
use axum::extract::Path;
use serde_json::json;

use crate::cognitive_memory::LibraryCognitiveMemory;
use crate::goal_curation::{GoalBoard, GoalProgress, save_goal_board};
use crate::operator_commands_dashboard::goals::*;
use crate::operator_commands_dashboard::{
    dashboard_goal_board_snapshot, dashboard_save_goal_board,
};
use crate::test_support::HermeticState;

/// Seed an empty goal board into the hermetic cognitive memory so handlers
/// that read from it don't fail on a missing snapshot.
fn init_empty_board(state: &HermeticState) {
    let mem = LibraryCognitiveMemory::open(state.state_root()).expect("open native memory");
    save_goal_board(&GoalBoard::new(), &mem).expect("seed empty board");
}

/// Seed a board with one active goal and one backlog item using the
/// dashboard helpers (same read/write path the handlers use).
fn init_board_with_goals(state: &HermeticState) {
    // Step 1: initialize cognitive memory DB
    {
        let mem = LibraryCognitiveMemory::open(state.state_root()).expect("open native memory");
        save_goal_board(&GoalBoard::new(), &mem).expect("init empty board");
    }

    // Step 2: save the actual board through the dashboard helpers
    let mut board = GoalBoard::new();
    board.active.push(crate::goal_curation::ActiveGoal {
        id: "existing-goal".to_string(),
        description: "An existing active goal".to_string(),
        priority: 1,
        status: GoalProgress::InProgress { percent: 50 },
        assigned_to: Some("simard".to_string()),
        current_activity: Some("doing stuff".to_string()),
        wip_refs: vec![],
        last_progress_update_at: None,
    });
    board.backlog.push(crate::goal_curation::BacklogItem {
        id: "backlog-item-1".to_string(),
        description: "A backlog item".to_string(),
        source: "test".to_string(),
        score: 0.8,
    });

    dashboard_save_goal_board(state.state_root(), &board).expect("seed board via dashboard helper");
}

// ---------------------------------------------------------------------------
// seed_goals
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn seed_goals_creates_initial_goals() {
    let state = HermeticState::new();
    init_empty_board(&state);

    let result = seed_goals().await;
    let val = &result.0;
    assert_eq!(val["status"], "ok");
    assert!(
        val["message"]
            .as_str()
            .unwrap_or("")
            .contains("3 active goals"),
        "expected seeding message, got: {val}"
    );

    // Verify the board was actually persisted
    let board =
        dashboard_goal_board_snapshot(state.state_root()).expect("should read back seeded board");
    assert_eq!(board.active.len(), 3);
    assert_eq!(board.backlog.len(), 2);
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn seed_goals_noop_when_already_seeded() {
    let state = HermeticState::new();
    init_empty_board(&state);
    let _ = seed_goals().await;

    let result = seed_goals().await;
    let val = &result.0;
    assert_eq!(val["status"], "already_seeded");
}

// ---------------------------------------------------------------------------
// add_goal — active
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn add_goal_creates_active_goal() {
    let state = HermeticState::new();
    init_empty_board(&state);

    let body = json!({"description": "A brand new goal", "priority": 2});
    let result = add_goal(Json(body)).await;
    let val = &result.0;
    assert_eq!(val["status"], "ok");
    assert!(val["id"].as_str().is_some(), "should return goal id");

    let board = dashboard_goal_board_snapshot(state.state_root()).unwrap();
    assert_eq!(board.active.len(), 1);
    assert_eq!(board.active[0].description, "A brand new goal");
    assert_eq!(board.active[0].priority, 2);
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn add_goal_defaults_priority_to_3() {
    let state = HermeticState::new();
    init_empty_board(&state);

    let body = json!({"description": "Goal without priority"});
    let _ = add_goal(Json(body)).await;

    let board = dashboard_goal_board_snapshot(state.state_root()).unwrap();
    assert_eq!(board.active[0].priority, 3);
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn add_goal_rejects_empty_description() {
    let state = HermeticState::new();
    init_empty_board(&state);

    let body = json!({"description": ""});
    let result = add_goal(Json(body)).await;
    let val = &result.0;
    assert!(val["error"].as_str().is_some(), "should return error");
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn add_goal_rejects_missing_description() {
    let state = HermeticState::new();
    init_empty_board(&state);

    let body = json!({"priority": 1});
    let result = add_goal(Json(body)).await;
    let val = &result.0;
    assert!(val["error"].as_str().unwrap().contains("description"));
}

// ---------------------------------------------------------------------------
// add_goal — backlog
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn add_goal_creates_backlog_item() {
    let state = HermeticState::new();
    init_empty_board(&state);

    let body = json!({"description": "Backlog idea", "type": "backlog", "score": 0.7});
    let result = add_goal(Json(body)).await;
    let val = &result.0;
    assert_eq!(val["status"], "ok");

    let board = dashboard_goal_board_snapshot(state.state_root()).unwrap();
    assert!(board.active.is_empty());
    assert_eq!(board.backlog.len(), 1);
    assert_eq!(board.backlog[0].description, "Backlog idea");
    assert!((board.backlog[0].score - 0.7).abs() < f64::EPSILON);
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn add_goal_backlog_defaults_score_to_half() {
    let state = HermeticState::new();
    init_empty_board(&state);

    let body = json!({"description": "No score backlog", "type": "backlog"});
    let _ = add_goal(Json(body)).await;

    let board = dashboard_goal_board_snapshot(state.state_root()).unwrap();
    assert!((board.backlog[0].score - 0.5).abs() < f64::EPSILON);
}

// ---------------------------------------------------------------------------
// add_goal — max active goals enforcement
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn add_goal_rejects_when_at_max_active() {
    let state = HermeticState::new();
    {
        let mem = LibraryCognitiveMemory::open(state.state_root()).expect("open");
        save_goal_board(&GoalBoard::new(), &mem).expect("init");
    }
    let mut board = GoalBoard::new();
    for i in 0..crate::goal_curation::MAX_ACTIVE_GOALS {
        board.active.push(crate::goal_curation::ActiveGoal {
            id: format!("max-goal-{i}"),
            description: format!("Max goal {i}"),
            priority: (i + 1) as u32,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
        });
    }
    dashboard_save_goal_board(state.state_root(), &board).expect("save full board");

    let body = json!({"description": "One too many"});
    let result = add_goal(Json(body)).await;
    let val = &result.0;
    assert!(
        val["error"].as_str().unwrap().contains("Maximum"),
        "expected max-goals error, got: {val}"
    );
}

// ---------------------------------------------------------------------------
// remove_goal
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn remove_goal_removes_active_goal() {
    let state = HermeticState::new();
    init_board_with_goals(&state);

    let result = remove_goal(Path("existing-goal".to_string())).await;
    assert_eq!(
        result.0["status"], "ok",
        "remove_goal should succeed for existing goal"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn remove_goal_removes_backlog_item() {
    let state = HermeticState::new();
    init_board_with_goals(&state);

    let result = remove_goal(Path("backlog-item-1".to_string())).await;
    assert_eq!(
        result.0["status"], "ok",
        "remove_goal should succeed for existing backlog item"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn remove_goal_returns_error_for_unknown_id() {
    let state = HermeticState::new();
    init_board_with_goals(&state);

    let result = remove_goal(Path("nonexistent".to_string())).await;
    let val = &result.0;
    assert!(val["error"].as_str().unwrap().contains("not found"));
}

// ---------------------------------------------------------------------------
// update_goal_status
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn update_goal_status_transitions_to_completed() {
    let state = HermeticState::new();
    init_board_with_goals(&state);

    let body = json!({"status": "completed"});
    let result = update_goal_status(Path("existing-goal".to_string()), Json(body)).await;
    let val = &result.0;
    assert_eq!(val["status"], "ok");

    let board = dashboard_goal_board_snapshot(state.state_root()).unwrap();
    assert!(
        matches!(board.active[0].status, GoalProgress::Completed),
        "expected Completed, got: {:?}",
        board.active[0].status
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn update_goal_status_blocked_with_reason() {
    let state = HermeticState::new();
    init_board_with_goals(&state);

    let body = json!({"status": "blocked", "reason": "waiting on PR review"});
    let result = update_goal_status(Path("existing-goal".to_string()), Json(body)).await;
    assert_eq!(result.0["status"], "ok");

    let board = dashboard_goal_board_snapshot(state.state_root()).unwrap();
    match &board.active[0].status {
        GoalProgress::Blocked(reason) => assert_eq!(reason, "waiting on PR review"),
        other => panic!("expected Blocked, got: {other:?}"),
    }
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn update_goal_status_all_valid_statuses() {
    let valid = [
        "proposed",
        "not-started",
        "in-progress",
        "paused",
        "completed",
    ];
    for status in valid {
        let state = HermeticState::new();
        init_board_with_goals(&state);
        let body = json!({"status": status});
        let result = update_goal_status(Path("existing-goal".to_string()), Json(body)).await;
        assert_eq!(
            result.0["status"], "ok",
            "status '{status}' should be accepted"
        );
    }
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn update_goal_status_rejects_unknown_status() {
    let state = HermeticState::new();
    init_board_with_goals(&state);

    let body = json!({"status": "bogus"});
    let result = update_goal_status(Path("existing-goal".to_string()), Json(body)).await;
    assert!(
        result.0["error"]
            .as_str()
            .unwrap()
            .contains("unknown status")
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn update_goal_status_requires_status_field() {
    let state = HermeticState::new();
    init_board_with_goals(&state);

    let body = json!({"reason": "no status"});
    let result = update_goal_status(Path("existing-goal".to_string()), Json(body)).await;
    assert!(result.0["error"].as_str().unwrap().contains("required"));
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn update_goal_status_returns_error_for_nonexistent_goal() {
    let state = HermeticState::new();
    init_board_with_goals(&state);

    let body = json!({"status": "completed"});
    let result = update_goal_status(Path("ghost".to_string()), Json(body)).await;
    assert!(result.0["error"].as_str().unwrap().contains("not found"));
}

// ---------------------------------------------------------------------------
// promote_backlog_item
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn promote_backlog_item_moves_to_active() {
    let state = HermeticState::new();
    init_board_with_goals(&state);

    let result = promote_backlog_item(Path("backlog-item-1".to_string())).await;
    let val = &result.0;
    assert_eq!(val["status"], "ok");

    let board = dashboard_goal_board_snapshot(state.state_root()).unwrap();
    assert_eq!(board.active.len(), 2);
    assert!(board.backlog.is_empty());
    let promoted = board.active.iter().find(|g| g.id == "backlog-item-1");
    assert!(promoted.is_some(), "promoted goal should be in active list");
    assert_eq!(
        promoted.unwrap().priority,
        3,
        "default priority should be 3"
    );
    assert!(
        matches!(promoted.unwrap().status, GoalProgress::NotStarted),
        "promoted goal should start as NotStarted"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn promote_backlog_item_returns_error_when_not_found() {
    let state = HermeticState::new();
    init_board_with_goals(&state);

    let result = promote_backlog_item(Path("no-such-item".to_string())).await;
    assert!(result.0["error"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn promote_backlog_item_rejects_when_at_max_active() {
    let state = HermeticState::new();
    {
        let mem = LibraryCognitiveMemory::open(state.state_root()).expect("open");
        save_goal_board(&GoalBoard::new(), &mem).expect("init");
    }
    let mut board = GoalBoard::new();
    for i in 0..crate::goal_curation::MAX_ACTIVE_GOALS {
        board.active.push(crate::goal_curation::ActiveGoal {
            id: format!("full-{i}"),
            description: format!("Full board goal {i}"),
            priority: (i + 1) as u32,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
        });
    }
    board.backlog.push(crate::goal_curation::BacklogItem {
        id: "want-to-promote".to_string(),
        description: "Should be rejected".to_string(),
        source: "test".to_string(),
        score: 0.5,
    });
    dashboard_save_goal_board(state.state_root(), &board).expect("save");

    let result = promote_backlog_item(Path("want-to-promote".to_string())).await;
    assert!(result.0["error"].as_str().unwrap().contains("Maximum"));
}

// ---------------------------------------------------------------------------
// demote_goal
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn demote_goal_moves_active_to_backlog() {
    let state = HermeticState::new();
    init_board_with_goals(&state);

    let result = demote_goal(Path("existing-goal".to_string())).await;
    let val = &result.0;
    assert_eq!(val["status"], "ok");

    let board = dashboard_goal_board_snapshot(state.state_root()).unwrap();
    assert!(board.active.is_empty());
    assert_eq!(board.backlog.len(), 2);
    let demoted = board.backlog.iter().find(|g| g.id == "existing-goal");
    assert!(demoted.is_some());
    assert_eq!(demoted.unwrap().source, "demoted");
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn demote_goal_returns_error_when_not_found() {
    let state = HermeticState::new();
    init_board_with_goals(&state);

    let result = demote_goal(Path("no-such-goal".to_string())).await;
    assert!(result.0["error"].as_str().unwrap().contains("not found"));
}

// ---------------------------------------------------------------------------
// goals (listing endpoint)
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn goals_returns_active_and_backlog_lists() {
    let state = HermeticState::new();
    init_board_with_goals(&state);

    let result = goals().await;
    let val = &result.0;

    assert_eq!(val["active_count"], 1);
    assert_eq!(val["backlog_count"].as_u64().unwrap_or(0), 1);

    let active = val["active"].as_array().unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0]["id"], "existing-goal");
    assert_eq!(active[0]["description"], "An existing active goal");
    assert!(
        active[0]["status_chip"].as_str().is_some(),
        "should have status_chip"
    );
    assert!(
        !active[0]["detail_full"].is_null(),
        "should have detail_full"
    );

    let backlog = val["backlog"].as_array().unwrap();
    assert!(!backlog.is_empty());
    let bl = backlog.iter().find(|b| b["id"] == "backlog-item-1");
    assert!(bl.is_some(), "backlog-item-1 should be in backlog list");
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn goals_returns_empty_when_no_board() {
    let state = HermeticState::new();
    {
        let _mem = LibraryCognitiveMemory::open(state.state_root()).expect("open");
    }

    let result = goals().await;
    let val = &result.0;
    assert_eq!(val["active_count"], 0);
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn goals_includes_status_chip_for_active_goals() {
    let state = HermeticState::new();
    {
        let mem = LibraryCognitiveMemory::open(state.state_root()).expect("open");
        save_goal_board(&GoalBoard::new(), &mem).expect("init");
    }
    let mut board = GoalBoard::new();
    board.active.push(crate::goal_curation::ActiveGoal {
        id: "chip-test".to_string(),
        description: "Goal with current_activity".to_string(),
        priority: 1,
        status: GoalProgress::InProgress { percent: 42 },
        assigned_to: None,
        current_activity: Some("advance-goal: opened PR #42".to_string()),
        wip_refs: vec![],
        last_progress_update_at: None,
    });
    dashboard_save_goal_board(state.state_root(), &board).expect("save");

    let result = goals().await;
    let active = result.0["active"].as_array().unwrap();
    assert_eq!(active[0]["status_chip"], "Working");
    assert!(
        active[0]["detail"]
            .as_str()
            .unwrap()
            .contains("opened PR #42")
    );
}

// ---------------------------------------------------------------------------
// Full CRUD workflow
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn full_goal_lifecycle_crud() {
    let state = HermeticState::new();
    init_empty_board(&state);

    // 1. Seed initial goals
    let r = seed_goals().await;
    assert_eq!(r.0["status"], "ok");

    // 2. Add a backlog item
    let r = add_goal(Json(
        json!({"description": "New backlog idea", "type": "backlog"}),
    ))
    .await;
    assert_eq!(r.0["status"], "ok");
    let backlog_id = r.0["id"].as_str().unwrap().to_string();

    // 3. Promote backlog to active
    let r = promote_backlog_item(Path(backlog_id.clone())).await;
    assert_eq!(r.0["status"], "ok");

    // 4. Update its status
    let r = update_goal_status(
        Path(backlog_id.clone()),
        Json(json!({"status": "in-progress"})),
    )
    .await;
    assert_eq!(r.0["status"], "ok");

    // 5. Mark complete
    let r = update_goal_status(
        Path(backlog_id.clone()),
        Json(json!({"status": "completed"})),
    )
    .await;
    assert_eq!(r.0["status"], "ok");

    // 6. Demote it back to backlog
    let r = demote_goal(Path(backlog_id.clone())).await;
    assert_eq!(r.0["status"], "ok");

    // 7. Remove it
    let r = remove_goal(Path(backlog_id)).await;
    assert_eq!(r.0["status"], "ok");

    // 8. Verify final state
    let board = dashboard_goal_board_snapshot(state.state_root()).unwrap();
    assert_eq!(
        board.active.len(),
        3,
        "only seeded goals should remain active"
    );
}

// -------------------------
