//! Tests for the goal CRUD handlers in `goals.rs` (issue #1750).
//!
//! Each test uses [`HermeticState`] to isolate cognitive-memory state and
//! calls the async handler functions directly with constructed axum
//! extractor wrappers.

use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use serde_json::json;

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::goal_curation::{GoalBoard, GoalProgress, save_goal_board};
use crate::memory_ipc::{clear_in_process_writer, register_in_process_writer};
use crate::operator_commands_dashboard::goals::*;
use crate::operator_commands_dashboard::{
    dashboard_goal_board_snapshot, dashboard_save_goal_board, register_dashboard_shared_writer,
};
use crate::test_support::HermeticState;

/// Holds the single shared in-process cognitive-memory writer for the duration
/// of a test and clears the global registration on drop (panic-safe).
///
/// Production wires the dashboard against ONE shared `LibraryCognitiveMemory`
/// handle: the OODA daemon, bootstrap assembly, and standalone `dashboard serve`
/// all open the store once and register it via [`register_in_process_writer`].
/// These handler tests must mirror that. Without a shared handle every
/// `launch_writer_bridge` / `open_reader_bridge` call opens a *fresh*
/// `LibraryCognitiveMemory`; because the lbug store holds an exclusive lock for
/// a handle's lifetime, a reopen races the previous handle's lock release / WAL
/// checkpoint and can observe an empty store — making these tests flaky and
/// diverging from the real production read-after-write path.
struct SharedMemoryGuard {
    writer: Arc<dyn CognitiveMemoryOps>,
}

impl SharedMemoryGuard {
    fn register(state: &HermeticState) -> Self {
        let writer: Arc<dyn CognitiveMemoryOps> = Arc::new(
            LibraryCognitiveMemory::open(state.state_root()).expect("open shared cognitive memory"),
        );
        register_in_process_writer(state.state_root().to_path_buf(), Arc::clone(&writer));
        Self { writer }
    }

    fn ops(&self) -> &dyn CognitiveMemoryOps {
        self.writer.as_ref()
    }
}

impl Drop for SharedMemoryGuard {
    fn drop(&mut self) {
        clear_in_process_writer();
    }
}

/// Seed an empty goal board into the hermetic cognitive memory so handlers
/// that read from it don't fail on a missing snapshot.
///
/// Returns the [`SharedMemoryGuard`] keeping the shared writer registered; the
/// caller MUST bind it (`let _mem = init_empty_board(&state);`) so every handler
/// call routes through the one shared handle.
#[must_use]
fn init_empty_board(state: &HermeticState) -> SharedMemoryGuard {
    let guard = SharedMemoryGuard::register(state);
    save_goal_board(&GoalBoard::new(), guard.ops()).expect("seed empty board");
    guard
}

/// Seed a board with one active goal and one backlog item using the
/// dashboard helpers (same read/write path the handlers use).
///
/// Returns the [`SharedMemoryGuard`]; the caller MUST bind it so the shared
/// writer stays registered for the life of the test.
#[must_use]
fn init_board_with_goals(state: &HermeticState) -> SharedMemoryGuard {
    let guard = SharedMemoryGuard::register(state);
    // Initialize the cognitive memory DB through the shared handle.
    save_goal_board(&GoalBoard::new(), guard.ops()).expect("init empty board");

    // Save the actual board through the dashboard helpers (tier-0 shared writer).
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
    guard
}

// ---------------------------------------------------------------------------
// seed_goals
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn seed_goals_creates_initial_goals() {
    let state = HermeticState::new();
    let _mem = init_empty_board(&state);

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
    let _mem = init_empty_board(&state);
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
    let _mem = init_empty_board(&state);

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
    let _mem = init_empty_board(&state);

    let body = json!({"description": "Goal without priority"});
    let _ = add_goal(Json(body)).await;

    let board = dashboard_goal_board_snapshot(state.state_root()).unwrap();
    assert_eq!(board.active[0].priority, 3);
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn add_goal_rejects_empty_description() {
    let state = HermeticState::new();
    let _mem = init_empty_board(&state);

    let body = json!({"description": ""});
    let result = add_goal(Json(body)).await;
    let val = &result.0;
    assert!(val["error"].as_str().is_some(), "should return error");
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn add_goal_rejects_missing_description() {
    let state = HermeticState::new();
    let _mem = init_empty_board(&state);

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
    let _mem = init_empty_board(&state);

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
    let _mem = init_empty_board(&state);

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
    let mem = SharedMemoryGuard::register(&state);
    save_goal_board(&GoalBoard::new(), mem.ops()).expect("init");
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
    let _mem = init_board_with_goals(&state);

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
    let _mem = init_board_with_goals(&state);

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
    let _mem = init_board_with_goals(&state);

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
    let _mem = init_board_with_goals(&state);

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
    let _mem = init_board_with_goals(&state);

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
        let _mem = init_board_with_goals(&state);
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
    let _mem = init_board_with_goals(&state);

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
    let _mem = init_board_with_goals(&state);

    let body = json!({"reason": "no status"});
    let result = update_goal_status(Path("existing-goal".to_string()), Json(body)).await;
    assert!(result.0["error"].as_str().unwrap().contains("required"));
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn update_goal_status_returns_error_for_nonexistent_goal() {
    let state = HermeticState::new();
    let _mem = init_board_with_goals(&state);

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
    let _mem = init_board_with_goals(&state);

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
    let _mem = init_board_with_goals(&state);

    let result = promote_backlog_item(Path("no-such-item".to_string())).await;
    assert!(result.0["error"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn promote_backlog_item_rejects_when_at_max_active() {
    let state = HermeticState::new();
    let mem = SharedMemoryGuard::register(&state);
    save_goal_board(&GoalBoard::new(), mem.ops()).expect("init");
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
    let _mem = init_board_with_goals(&state);

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
    let _mem = init_board_with_goals(&state);

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
    let _mem = init_board_with_goals(&state);

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
    // Register the shared writer but seed no board — goals() must report empty.
    let _mem = SharedMemoryGuard::register(&state);

    let result = goals().await;
    let val = &result.0;
    assert_eq!(val["active_count"], 0);
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn goals_includes_status_chip_for_active_goals() {
    let state = HermeticState::new();
    let mem = SharedMemoryGuard::register(&state);
    save_goal_board(&GoalBoard::new(), mem.ops()).expect("init");
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
    let _mem = init_empty_board(&state);

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
// Shared-writer wiring (regression for fresh-open goal-board data loss)
// -------------------------

/// Regression for the silent goal-board data-loss race, exercised through the
/// real handler path (each `add_goal` does load→modify→save, exactly like the
/// production dashboard). With the shared in-process writer registered — as the
/// daemon, bootstrap, and standalone `dashboard serve` all do — every write
/// must persist. Before the fix, dashboard handlers opened a *fresh*
/// `LibraryCognitiveMemory` per call; the lbug store's exclusive per-handle lock
/// meant a reopen could race the previous handle's release and read an empty
/// board, after which the next save persisted that empty board and silently
/// dropped every goal.
#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn repeated_handler_writes_never_silently_drop_goals() {
    let state = HermeticState::new();
    let _mem = init_empty_board(&state);

    for i in 0..12 {
        let r = add_goal(Json(json!({
            "description": format!("Ship reliability fix for module {i}"),
            "type": "backlog",
        })))
        .await;
        assert_eq!(r.0["status"], "ok", "add_goal {i} must succeed");
    }

    let board = dashboard_goal_board_snapshot(state.state_root()).unwrap();
    assert_eq!(
        board.backlog.len(),
        12,
        "every goal added through the handler must persist (no silent drop)"
    );
}

/// `register_dashboard_shared_writer` returns a live writer for a writable
/// state root, and the registered handle services subsequent bridge opens.
#[test]
#[serial_test::serial(cognitive_memory)]
fn register_dashboard_shared_writer_registers_usable_handle() {
    let state = HermeticState::new();
    let writer = register_dashboard_shared_writer(state.state_root());
    assert!(
        writer.is_some(),
        "a fresh hermetic state root must yield a usable cognitive-memory writer"
    );

    // The registered writer must serve a save+read through the dashboard path.
    dashboard_save_goal_board(state.state_root(), &GoalBoard::new()).expect("save via shared");
    let board = dashboard_goal_board_snapshot(state.state_root()).expect("read via shared");
    assert!(board.active.is_empty());

    drop(writer);
    clear_in_process_writer();
}
