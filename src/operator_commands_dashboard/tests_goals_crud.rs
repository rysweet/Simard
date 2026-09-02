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
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, save_goal_board};
use crate::memory_ipc::{clear_in_process_writer, register_in_process_writer};
use crate::operator_commands_dashboard::goals::*;
use crate::operator_commands_dashboard::{
    dashboard_goal_board_snapshot, dashboard_save_goal_board,
};
use crate::test_support::HermeticState;

/// Holds the single shared in-process cognitive-memory writer for the duration
/// of a test and clears the global registration on drop (panic-safe).
///
/// Production wires the dashboard against ONE shared `LibraryCognitiveMemory`
/// handle registered as the tier-0 in-process writer: the OODA daemon, bootstrap
/// assembly, and standalone `dashboard serve` all open the store once and
/// register it via [`register_in_process_writer`]. These handler tests mirror
/// that tier-0 wiring so they exercise the same read/write path production uses.
///
/// Same-process read-after-write consistency is also guaranteed at the
/// launcher's tier-2 store cache (`shared_tier2_store`, added in #2334 to close
/// the #2320 goal-board read-after-write race), so the handlers persist
/// correctly even without an explicit tier-0 registration. Registering the
/// shared writer here keeps the tests aligned with the production tier-0 path
/// rather than silently relying on the tier-2 fallback.
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
        labels: Vec::new(),
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
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
            labels: Vec::new(),
            parent_goal_id: None,
            priority_explicit: false,
            repo: None,
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
            labels: Vec::new(),
            parent_goal_id: None,
            priority_explicit: false,
            repo: None,
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
        labels: Vec::new(),
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
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

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn goals_api_exposes_labels_and_omits_them_when_empty() {
    // Issue #2743: /api/goals additively exposes each goal's labels array, and
    // omits the key for an unlabelled goal (mirrors the serde skip contract).
    let state = HermeticState::new();
    let mem = SharedMemoryGuard::register(&state);
    save_goal_board(&GoalBoard::new(), mem.ops()).expect("init");
    let mut board = GoalBoard::new();
    board.active.push(
        crate::goal_curation::ActiveGoal::new("tagged", "A labelled goal", 1)
            .with_label("source:creative-ideas")
            .with_label("area:dashboard"),
    );
    board.active.push(crate::goal_curation::ActiveGoal::new(
        "bare",
        "An unlabelled goal",
        2,
    ));
    dashboard_save_goal_board(state.state_root(), &board).expect("save");

    let result = goals().await;
    let active = result.0["active"].as_array().unwrap();
    // Goals are sorted by priority asc: "tagged" (p1) then "bare" (p2).
    let tagged = active
        .iter()
        .find(|g| g["id"] == "tagged")
        .expect("tagged goal present");
    let labels = tagged["labels"].as_array().expect("labels array present");
    let labels: Vec<&str> = labels.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(labels, vec!["source:creative-ideas", "area:dashboard"]);

    let bare = active
        .iter()
        .find(|g| g["id"] == "bare")
        .expect("bare goal present");
    assert!(
        bare.get("labels").is_none(),
        "an unlabelled goal omits the labels key: {bare}",
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

/// Exercises the real handler path (each `add_goal` does load→modify→save,
/// exactly like the production dashboard) and asserts every write persists with
/// no silent drop. The silent goal-board data-loss class (#1590 / #2320) came
/// from per-call fresh `LibraryCognitiveMemory` opens racing the lbug store's
/// exclusive per-handle lock: a reopen could read an empty board and the next
/// save would persist that empty board, dropping every goal. That race is now
/// prevented both by the shared tier-0 in-process writer the
/// daemon/bootstrap/`dashboard serve` register and by the launcher's tier-2
/// store cache (#2334). With the tier-0 writer registered (as production does),
/// every handler call short-circuits at tier-0, so this test guards the
/// handler-level persistence contract on the tier-0 path the dashboard uses.
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

// ===========================================================================
// PR B — issues #2408 / #2384: env-free `_at` cores (state-root isolation)
// ===========================================================================
//
// Root cause of the `full_goal_lifecycle_crud` /
// `operator_commands_dashboard::tests_goals_crud::full_*` flake: the goal-CRUD
// handlers resolve their state root *ambiently* via `resolve_state_root()` →
// `std::env::var("SIMARD_STATE_ROOT")`. glibc getenv/setenv are not
// thread-safe, so a concurrent env mutation in ANOTHER test — even in a
// different file of the same lib-test binary — can tear a handler's read
// mid-flight and send writes to `$HOME/.simard` instead of the hermetic temp
// root, corrupting the board the test then reads back. `add_goal` is doubly
// exposed: it resolves the env once directly AND again inside
// `load_board_or_empty`.
//
// The fix threads an EXPLICIT `state_root: &Path` through each handler via an
// `*_at` core; the ambient `resolve_state_root()` survives only in a thin
// wrapper, and `add_goal`'s second resolve is killed with `load_board_or_empty_at`.
//
// These tests pin that contract. Each registers a shared writer + seeds a
// board at an explicit `target` root, then constructs a SECOND `HermeticState`
// so the ambient `SIMARD_STATE_ROOT` points at a `decoy` directory that DIFFERS
// from `target`. A correct env-free `_at` core uses the explicit `target` it is
// handed; an ambient one would touch the `decoy`, and the assertions below
// fail.
//
// Local-binding drop order makes this sound: bindings drop in reverse
// declaration order, so `decoy` (declared last) restores `SIMARD_STATE_ROOT`
// to `target`'s value, then the writer guard clears, then `target` restores the
// original env — no stale pin leaks past the test.
//
// This is the TDD red phase: the `*_at` functions do not exist yet, so the
// lib-test binary will not compile until #2408/#2384's fix lands. Once the
// cores exist and honor their explicit root, every assertion passes under
// parallel `cargo test`.

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn seed_goals_at_writes_to_explicit_root_not_ambient_env() {
    let target = HermeticState::new();
    let _mem = init_empty_board(&target);
    let decoy = HermeticState::new(); // ambient SIMARD_STATE_ROOT now != target

    let r = seed_goals_at(target.state_root()).await;
    assert_eq!(
        r.0["status"], "ok",
        "seed_goals_at must seed the explicit root, got: {}",
        r.0
    );

    let board = dashboard_goal_board_snapshot(target.state_root()).unwrap();
    assert_eq!(
        board.active.len(),
        3,
        "seeded goals must land in the explicit target root"
    );

    let decoy_board = dashboard_goal_board_snapshot(decoy.state_root()).unwrap_or_default();
    assert!(
        decoy_board.active.is_empty() && decoy_board.backlog.is_empty(),
        "seed_goals_at must NOT touch the ambient SIMARD_STATE_ROOT (decoy) root"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn add_goal_at_loads_and_saves_explicit_root_not_ambient_env() {
    // Guards the #2408 double-resolution in `add_goal`: BOTH the load
    // (`load_board_or_empty_at`) and the save must honor the explicit root.
    let target = HermeticState::new();
    let _mem = init_board_with_goals(&target); // 1 active "existing-goal" + 1 backlog
    let decoy = HermeticState::new();

    let r = add_goal_at(
        target.state_root(),
        Json(json!({"description": "Explicit root goal", "type": "backlog"})),
    )
    .await;
    assert_eq!(
        r.0["status"], "ok",
        "add_goal_at must succeed, got: {}",
        r.0
    );

    let board = dashboard_goal_board_snapshot(target.state_root()).unwrap();
    // Pre-existing active goal preserved => the LOAD read the explicit root,
    // not the empty decoy (an ambient load would drop "existing-goal").
    assert_eq!(
        board.active.len(),
        1,
        "existing active goal must survive — load must honor the explicit root"
    );
    // Seeded backlog item + the new one => the SAVE wrote the explicit root.
    assert_eq!(
        board.backlog.len(),
        2,
        "new backlog item must persist to the explicit root alongside the seeded one"
    );

    let decoy_board = dashboard_goal_board_snapshot(decoy.state_root()).unwrap_or_default();
    assert!(
        decoy_board.active.is_empty() && decoy_board.backlog.is_empty(),
        "add_goal_at must NOT touch the ambient (decoy) root"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn remove_goal_at_uses_explicit_root() {
    let target = HermeticState::new();
    let _mem = init_board_with_goals(&target);
    let _decoy = HermeticState::new();

    // If remove loaded from the empty decoy it would 404; success proves the
    // load read the explicit root.
    let r = remove_goal_at(target.state_root(), Path("existing-goal".to_string())).await;
    assert_eq!(
        r.0["status"], "ok",
        "remove_goal_at must find+remove via the explicit root, got: {}",
        r.0
    );

    let board = dashboard_goal_board_snapshot(target.state_root()).unwrap();
    assert!(
        board.active.iter().all(|g| g.id != "existing-goal"),
        "removed goal must be gone from the explicit root"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn update_goal_status_at_uses_explicit_root() {
    let target = HermeticState::new();
    let _mem = init_board_with_goals(&target);
    let _decoy = HermeticState::new();

    let r = update_goal_status_at(
        target.state_root(),
        Path("existing-goal".to_string()),
        Json(json!({"status": "completed"})),
    )
    .await;
    assert_eq!(
        r.0["status"], "ok",
        "update_goal_status_at must update via the explicit root, got: {}",
        r.0
    );

    let board = dashboard_goal_board_snapshot(target.state_root()).unwrap();
    assert!(
        matches!(board.active[0].status, GoalProgress::Completed),
        "status change must persist to the explicit root; got {:?}",
        board.active[0].status
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn promote_backlog_item_at_uses_explicit_root() {
    let target = HermeticState::new();
    let _mem = init_board_with_goals(&target); // backlog "backlog-item-1"
    let _decoy = HermeticState::new();

    let r = promote_backlog_item_at(target.state_root(), Path("backlog-item-1".to_string())).await;
    assert_eq!(
        r.0["status"], "ok",
        "promote_backlog_item_at must find the backlog item via the explicit root, got: {}",
        r.0
    );

    let board = dashboard_goal_board_snapshot(target.state_root()).unwrap();
    assert!(
        board.active.iter().any(|g| g.id == "backlog-item-1"),
        "promoted item must be active in the explicit root"
    );
    assert!(
        board.backlog.iter().all(|g| g.id != "backlog-item-1"),
        "promoted item must leave the backlog in the explicit root"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn demote_goal_at_uses_explicit_root() {
    let target = HermeticState::new();
    let _mem = init_board_with_goals(&target); // active "existing-goal"
    let _decoy = HermeticState::new();

    let r = demote_goal_at(target.state_root(), Path("existing-goal".to_string())).await;
    assert_eq!(
        r.0["status"], "ok",
        "demote_goal_at must find the active goal via the explicit root, got: {}",
        r.0
    );

    let board = dashboard_goal_board_snapshot(target.state_root()).unwrap();
    assert!(
        board.backlog.iter().any(|g| g.id == "existing-goal"),
        "demoted goal must be in the backlog in the explicit root"
    );
    assert!(
        board.active.iter().all(|g| g.id != "existing-goal"),
        "demoted goal must leave the active list in the explicit root"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn goals_at_reads_explicit_root_not_ambient_env() {
    let target = HermeticState::new();
    let _mem = init_board_with_goals(&target);
    let _decoy = HermeticState::new();

    let r = goals_at(target.state_root()).await;
    let v = &r.0;
    assert_eq!(
        v["active_count"], 1,
        "goals_at must read the explicit root's active goals, got: {v}"
    );
    assert!(
        v["active"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["id"] == "existing-goal"),
        "goals_at must surface the seeded goal from the explicit root, got: {v}"
    );
}

/// Deterministic, env-independent twin of `full_goal_lifecycle_crud` (the
/// #2408 / #2384 flake): drive the entire CRUD lifecycle through the
/// explicit-root `_at` cores while the ambient `SIMARD_STATE_ROOT` points at a
/// DECOY directory. A correct fix makes every step touch only `target`.
#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn full_goal_lifecycle_crud_via_at_is_env_independent() {
    let target = HermeticState::new();
    let _mem = init_empty_board(&target);
    let decoy = HermeticState::new();
    let root = target.state_root();

    // 1. Seed initial goals.
    assert_eq!(seed_goals_at(root).await.0["status"], "ok");

    // 2. Add a backlog item.
    let r = add_goal_at(
        root,
        Json(json!({"description": "New backlog idea", "type": "backlog"})),
    )
    .await;
    assert_eq!(r.0["status"], "ok");
    let backlog_id = r.0["id"].as_str().unwrap().to_string();

    // 3. Promote backlog -> active.
    assert_eq!(
        promote_backlog_item_at(root, Path(backlog_id.clone()))
            .await
            .0["status"],
        "ok"
    );

    // 4. In-progress.
    assert_eq!(
        update_goal_status_at(
            root,
            Path(backlog_id.clone()),
            Json(json!({"status": "in-progress"})),
        )
        .await
        .0["status"],
        "ok"
    );

    // 5. Completed.
    assert_eq!(
        update_goal_status_at(
            root,
            Path(backlog_id.clone()),
            Json(json!({"status": "completed"})),
        )
        .await
        .0["status"],
        "ok"
    );

    // 6. Demote back to backlog.
    assert_eq!(
        demote_goal_at(root, Path(backlog_id.clone())).await.0["status"],
        "ok"
    );

    // 7. Remove.
    assert_eq!(
        remove_goal_at(root, Path(backlog_id)).await.0["status"],
        "ok"
    );

    // 8. Only the 3 seeded goals remain, all in the explicit root.
    let board = dashboard_goal_board_snapshot(root).unwrap();
    assert_eq!(
        board.active.len(),
        3,
        "only the seeded goals should remain active in the explicit root"
    );

    // The ambient/decoy root must never have been written.
    let decoy_board = dashboard_goal_board_snapshot(decoy.state_root()).unwrap_or_default();
    assert!(
        decoy_board.active.is_empty() && decoy_board.backlog.is_empty(),
        "no CRUD step may touch the ambient (decoy) SIMARD_STATE_ROOT"
    );
}

// ===========================================================================
// Lifecycle status projection (issue #20)
//
// BUG: the dashboard "Goals" tab rendered EVERY active goal as failed/blocked
// even though `simard goal list` (ground truth) shows the goals in MIXED
// states — several `blocked` (with an OODA-safeguard "needs human review"
// reason), many `not-started`, and several `completed`.
//
// FIX (backend half): `/api/goals` additively exposes a `status_progress`
// field on each active goal — the *serialized `GoalProgress` enum* — so the
// client can render a distinct, correctly-labeled lifecycle badge (and surface
// the block reason) instead of dumping the free-form `status` string that,
// paired with the red activity chip, read as "failed" for every goal. The
// existing `status`, `status_chip`, `detail`, and `detail_full` fields are
// UNCHANGED (additive-only).
//
// These are the Rust/backend half of the contract; the frontend rendering
// half lives in `index_html/tests_tab_meta.rs`.
// ===========================================================================

/// The exact OODA-safeguard block reason from the confirmed diagnosis, used to
/// prove a genuinely-blocked goal surfaces its reason (not a blanket "failed").
const OODA_BLOCK_REASON: &str =
    "🔒 [OODA-SAFEGUARD] agent-kgpacks-rs: 3 consecutive no-action cycles; needs human review";

/// Build an active goal in an explicit lifecycle `status` with no
/// `current_activity` (so the lifecycle status — not the activity chip — is the
/// signal under test).
fn goal_in_status(id: &str, status: GoalProgress) -> crate::goal_curation::ActiveGoal {
    crate::goal_curation::ActiveGoal {
        labels: Vec::new(),
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
        id: id.to_string(),
        description: format!("Goal {id}"),
        priority: 2,
        status,
        assigned_to: Some("simard".to_string()),
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    }
}

/// Seed a board whose active goals span the four distinct lifecycle states the
/// Goals tab must render differently: not-started, in-progress, blocked (with a
/// reason), and completed. Returns the `SharedMemoryGuard` (bind it).
#[must_use]
fn init_board_with_mixed_statuses(state: &HermeticState) -> SharedMemoryGuard {
    let guard = SharedMemoryGuard::register(state);
    save_goal_board(&GoalBoard::new(), guard.ops()).expect("init empty board");

    let mut board = GoalBoard::new();
    board
        .active
        .push(goal_in_status("goal-not-started", GoalProgress::NotStarted));
    board.active.push(goal_in_status(
        "goal-in-progress",
        GoalProgress::InProgress { percent: 37 },
    ));
    board.active.push(goal_in_status(
        "goal-blocked",
        GoalProgress::Blocked(OODA_BLOCK_REASON.to_string()),
    ));
    board
        .active
        .push(goal_in_status("goal-completed", GoalProgress::Completed));

    dashboard_save_goal_board(state.state_root(), &board).expect("seed mixed-status board");
    guard
}

/// Find the active-goal JSON object with the given `id` in a `/api/goals`
/// response, panicking with context if absent.
fn active_by_id<'a>(active: &'a [serde_json::Value], id: &str) -> &'a serde_json::Value {
    active
        .iter()
        .find(|g| g["id"] == id)
        .unwrap_or_else(|| panic!("active goal {id:?} missing from /api/goals response"))
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn goals_exposes_distinct_status_progress_per_lifecycle_state() {
    let state = HermeticState::new();
    let _mem = init_board_with_mixed_statuses(&state);

    let result = goals_at(state.state_root()).await;
    let active = result.0["active"].as_array().expect("active array");
    assert_eq!(active.len(), 4, "all four seeded goals must be returned");

    // Each goal exposes an additive `status_progress` = the SERIALIZED
    // `GoalProgress` enum, distinctly per lifecycle state.
    assert_eq!(
        active_by_id(active, "goal-not-started")["status_progress"],
        json!("NotStarted"),
        "not-started goal must expose status_progress==\"NotStarted\", not a blocked/failed value"
    );
    assert_eq!(
        active_by_id(active, "goal-in-progress")["status_progress"],
        json!({ "InProgress": { "percent": 37 } }),
        "in-progress goal must expose the InProgress percent variant"
    );
    assert_eq!(
        active_by_id(active, "goal-blocked")["status_progress"],
        json!({ "Blocked": OODA_BLOCK_REASON }),
        "blocked goal must expose the Blocked variant carrying its reason"
    );
    assert_eq!(
        active_by_id(active, "goal-completed")["status_progress"],
        json!("Completed"),
        "completed goal must expose status_progress==\"Completed\", not a blocked/failed value"
    );

    // The four values must be DISTINCT — the bug rendered everything the same
    // (failed/blocked). Distinctness proves each real status is preserved.
    let mut seen = std::collections::HashSet::new();
    for g in active {
        let sp = g["status_progress"].to_string();
        assert!(
            seen.insert(sp.clone()),
            "status_progress values must be DISTINCT per lifecycle state; duplicate: {sp}"
        );
    }
    assert_eq!(
        seen.len(),
        4,
        "expected four distinct status_progress values, got {seen:?}"
    );

    // Direct regression guard: goals that are NOT blocked must not serialize as
    // a Blocked variant (the exact symptom of the bug).
    for id in ["goal-not-started", "goal-in-progress", "goal-completed"] {
        assert!(
            active_by_id(active, id)["status_progress"]
                .get("Blocked")
                .is_none(),
            "non-blocked goal {id:?} must never carry a Blocked status_progress"
        );
    }
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn goals_blocked_status_progress_carries_reason() {
    let state = HermeticState::new();
    let _mem = init_board_with_mixed_statuses(&state);

    let result = goals_at(state.state_root()).await;
    let active = result.0["active"].as_array().expect("active array");
    let blocked = active_by_id(active, "goal-blocked");

    assert_eq!(
        blocked["status_progress"]["Blocked"], OODA_BLOCK_REASON,
        "a genuinely-blocked goal must surface its block REASON via status_progress"
    );
    // The reason text ("needs human review") must be present so the operator
    // can act on it — not swallowed into a generic "failed/blocked".
    assert!(
        blocked["status_progress"]["Blocked"]
            .as_str()
            .unwrap_or("")
            .contains("needs human review"),
        "the block reason must retain the OODA-safeguard 'needs human review' text, got: {}",
        blocked["status_progress"]
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn goals_status_progress_is_additive_not_replacing_existing_fields() {
    let state = HermeticState::new();
    let _mem = init_board_with_mixed_statuses(&state);

    let result = goals_at(state.state_root()).await;
    let active = result.0["active"].as_array().expect("active array");

    // The change is additive: the pre-existing free-form `status` string and
    // the `status_chip`/`detail_full` fields must still be present alongside
    // the new `status_progress`.
    for g in active {
        assert!(
            g["status"].as_str().is_some(),
            "existing free-form `status` string must remain (additive change), goal: {}",
            g["id"]
        );
        assert!(
            g["status_chip"].as_str().is_some(),
            "existing `status_chip` must remain (additive change), goal: {}",
            g["id"]
        );
        assert!(
            !g["status_progress"].is_null(),
            "new `status_progress` must be populated, goal: {}",
            g["id"]
        );
    }

    // The blocked goal's legacy `status` Display string is unchanged
    // ("blocked: <reason>"), proving we ADDED status_progress rather than
    // rewriting the existing field.
    let blocked = active_by_id(active, "goal-blocked");
    assert!(
        blocked["status"]
            .as_str()
            .unwrap_or("")
            .starts_with("blocked"),
        "legacy `status` string must stay the GoalProgress Display form, got: {}",
        blocked["status"]
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn goals_status_progress_matches_live_goal_store() {
    // R8 reconciliation: the dashboard's rendered lifecycle status for each
    // goal must equal the goal store's actual `GoalProgress` (dashboard ⇄
    // `simard goal list` agreement), read LIVE from the store.
    let state = HermeticState::new();
    let _mem = init_board_with_mixed_statuses(&state);

    // Ground truth: re-read the persisted board (the same source of truth the
    // CLI's `goal list` reports).
    let store = dashboard_goal_board_snapshot(state.state_root()).expect("read back board");

    let result = goals_at(state.state_root()).await;
    let active = result.0["active"].as_array().expect("active array");
    assert_eq!(
        active.len(),
        store.active.len(),
        "dashboard active-goal count must match the goal store"
    );

    for stored in &store.active {
        let rendered = active_by_id(active, &stored.id);
        let expected = serde_json::to_value(&stored.status).expect("serialize GoalProgress");
        assert_eq!(
            rendered["status_progress"], expected,
            "goal {:?}: dashboard status_progress must equal the store's live GoalProgress",
            stored.id
        );
    }
}

// ===========================================================================
// Issue #2695 follow-up — Goal HIERARCHY + differentiated PRIORITY (backend).
//
// The Goals tab must (1) represent the parent→child decomposition hierarchy and
// (2) surface + order goals by priority so priority is visible and actionable.
// This backend half of the contract pins the `/api/goals` shape:
//   * active goals ordered by priority ASCENDING (p1 = highest first), with a
//     stable id tiebreak, so the client renders a priority-ordered tree;
//   * each goal additively exposes `parent_goal_id` (structured hierarchy edge,
//     G3 — no brittle parsing) and `priority_explicit` (operator-set provenance);
//   * the create path validates priority (>=1, no silent p0) and server-derives
//     `priority_explicit` (a client cannot forge operator-set provenance).
//
// The frontend rendering half lives in `index_html/tests_tab_meta.rs`; the
// prioritization-pass substance lives in `goal_curation/tests_prioritize.rs`.
// These are RED until `goals_at` orders + emits the new fields, `add_goal_at`
// validates + derives provenance, and `ActiveGoal.priority_explicit` exists.
// ===========================================================================

/// Seed a hermetic board with the given active goals through the same tier-0
/// shared-writer path the handlers use. Returns the `SharedMemoryGuard` (bind
/// it so the writer stays registered for the life of the test).
#[must_use]
fn seed_active_board(state: &HermeticState, goals: Vec<ActiveGoal>) -> SharedMemoryGuard {
    let guard = SharedMemoryGuard::register(state);
    save_goal_board(&GoalBoard::new(), guard.ops()).expect("init empty board");
    let mut board = GoalBoard::new();
    board.active = goals;
    dashboard_save_goal_board(state.state_root(), &board).expect("seed active board");
    guard
}

/// The ordered list of active-goal ids in a `/api/goals` response.
fn active_ids(active: &[serde_json::Value]) -> Vec<String> {
    active
        .iter()
        .map(|g| g["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn goals_active_ordered_by_priority_ascending_with_stable_id_tiebreak() {
    // Seeded deliberately OUT of priority order, with a duplicate priority to
    // exercise the stable id tiebreak.
    let state = HermeticState::new();
    let _mem = seed_active_board(
        &state,
        vec![
            ActiveGoal::new("b-goal", "priority 2, id b", 2),
            ActiveGoal::new("a-goal", "priority 2, id a", 2),
            ActiveGoal::new("c-goal", "priority 1, id c", 1),
            ActiveGoal::new("d-goal", "priority 4, id d", 4),
        ],
    );

    let result = goals_at(state.state_root()).await;
    let active = result.0["active"].as_array().expect("active array");

    // Highest-priority-first = ascending numeric priority; equal priorities keep
    // a stable ascending-id order.
    assert_eq!(
        active_ids(active),
        vec![
            "c-goal".to_string(), // p1
            "a-goal".to_string(), // p2, id 'a' before 'b'
            "b-goal".to_string(), // p2
            "d-goal".to_string(), // p4
        ],
        "/api/goals must return active goals ordered by priority ascending \
         (highest first) with a stable id tiebreak"
    );

    // The emitted priority sequence must itself be non-decreasing.
    let priorities: Vec<u64> = active
        .iter()
        .map(|g| g["priority"].as_u64().unwrap_or(0))
        .collect();
    assert!(
        priorities.windows(2).all(|w| w[0] <= w[1]),
        "emitted priorities must be non-decreasing (priority is visible AND ordered); got {priorities:?}"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn goals_expose_parent_goal_id_for_hierarchy() {
    // A decomposition: parent "umbrella" with one child "sub-task".
    let state = HermeticState::new();
    let _mem = seed_active_board(
        &state,
        vec![
            ActiveGoal::new("umbrella", "the parent goal", 1),
            ActiveGoal::new("sub-task", "a decomposed child", 2)
                .with_parent(Some("umbrella".to_string())),
        ],
    );

    let result = goals_at(state.state_root()).await;
    let active = result.0["active"].as_array().expect("active array");

    let child = active_by_id(active, "sub-task");
    assert_eq!(
        child["parent_goal_id"],
        json!("umbrella"),
        "a child goal must additively expose its parent_goal_id so the tab can nest it \
         under its parent (structured hierarchy edge, not brittle parsing)"
    );

    let parent = active_by_id(active, "umbrella");
    assert_eq!(
        parent["parent_goal_id"],
        json!(null),
        "a top-level goal must expose a null parent_goal_id (it roots the tree)"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn goals_expose_priority_explicit_provenance() {
    // One operator-pinned goal, one ordinary (pass-eligible) goal.
    let state = HermeticState::new();
    let _mem = seed_active_board(
        &state,
        vec![
            ActiveGoal::new("pinned", "operator pinned", 1).with_priority_explicit(true),
            ActiveGoal::new("ordinary", "not pinned", 3),
        ],
    );

    let result = goals_at(state.state_root()).await;
    let active = result.0["active"].as_array().expect("active array");

    assert_eq!(
        active_by_id(active, "pinned")["priority_explicit"],
        json!(true),
        "an operator-pinned goal must expose priority_explicit==true so the pass leaves it alone"
    );
    assert_eq!(
        active_by_id(active, "ordinary")["priority_explicit"],
        json!(false),
        "an ordinary goal must expose priority_explicit==false (eligible for differentiation)"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn add_goal_at_rejects_zero_priority() {
    // SR-V1: the create path must validate priority (>=1) rather than silently
    // persisting a p0 goal.
    let state = HermeticState::new();
    let _mem = init_empty_board(&state);

    let resp = add_goal_at(
        state.state_root(),
        Json(json!({ "description": "invalid zero priority", "priority": 0 })),
    )
    .await;

    assert!(
        resp.0.get("error").is_some(),
        "adding a goal with priority 0 must return an error (priority must be >= 1), got: {}",
        resp.0
    );

    // And no p0 goal may have been persisted.
    let listed = goals_at(state.state_root()).await;
    let active = listed.0["active"].as_array().expect("active array");
    assert!(
        active
            .iter()
            .all(|g| g["priority"].as_u64().unwrap_or(0) >= 1),
        "no active goal may carry priority 0 after a rejected add"
    );
}

#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn add_goal_at_ignores_client_supplied_priority_explicit() {
    // SR-V2: `priority_explicit` is server-derived provenance. A dashboard-added
    // goal is NOT operator-set-priority, so it must remain non-explicit even if a
    // client tries to forge the flag — otherwise a client could exempt a goal
    // from differentiation.
    let state = HermeticState::new();
    let _mem = init_empty_board(&state);

    let resp = add_goal_at(
        state.state_root(),
        Json(json!({
            "description": "client tries to forge provenance",
            "priority": 3,
            "priority_explicit": true
        })),
    )
    .await;
    assert_eq!(
        resp.0["status"], "ok",
        "add should succeed, got: {}",
        resp.0
    );

    let listed = goals_at(state.state_root()).await;
    let active = listed.0["active"].as_array().expect("active array");
    assert_eq!(active.len(), 1, "exactly one goal should have been added");
    assert_eq!(
        active[0]["priority_explicit"],
        json!(false),
        "a client-supplied priority_explicit must be IGNORED; dashboard-added goals stay \
         non-explicit (server-derived provenance)"
    );
}
