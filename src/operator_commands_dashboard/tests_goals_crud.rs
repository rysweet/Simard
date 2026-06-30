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
        parent_goal_id: None,
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
            parent_goal_id: None,
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
            parent_goal_id: None,
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
        parent_goal_id: None,
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
