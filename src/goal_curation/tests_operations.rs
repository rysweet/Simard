use super::operations::*;
use super::types::{ActiveGoal, BacklogItem, GoalBoard, GoalProgress, MAX_ACTIVE_GOALS};

fn make_goal(id: &str, priority: u32) -> ActiveGoal {
    ActiveGoal {
        id: id.to_string(),
        description: format!("Goal {id}"),
        priority,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    }
}

fn make_backlog(id: &str) -> BacklogItem {
    BacklogItem {
        id: id.to_string(),
        description: format!("Backlog {id}"),
        source: "test".to_string(),
        score: 0.0,
    }
}

#[test]
fn add_active_goal_succeeds_and_rejects_duplicate() {
    let mut board = GoalBoard::new();
    assert!(add_active_goal(&mut board, make_goal("g1", 1)).is_ok());
    assert_eq!(board.active.len(), 1);
    assert!(add_active_goal(&mut board, make_goal("g1", 2)).is_err());
}

#[test]
fn add_active_goal_rejects_at_capacity() {
    let mut board = GoalBoard::new();
    for i in 0..MAX_ACTIVE_GOALS {
        add_active_goal(&mut board, make_goal(&format!("g{i}"), (i + 1) as u32)).unwrap();
    }
    let result = add_active_goal(&mut board, make_goal("overflow", 1));
    assert!(result.is_err());
}

#[test]
fn add_active_goal_rejects_zero_priority() {
    let mut board = GoalBoard::new();
    let result = add_active_goal(&mut board, make_goal("g1", 0));
    assert!(result.is_err());
}

#[test]
fn add_backlog_item_succeeds_and_rejects_duplicate() {
    let mut board = GoalBoard::new();
    assert!(add_backlog_item(&mut board, make_backlog("b1")).is_ok());
    assert_eq!(board.backlog.len(), 1);
    assert!(add_backlog_item(&mut board, make_backlog("b1")).is_err());
}

#[test]
fn promote_to_active_moves_item() {
    let mut board = GoalBoard::new();
    add_backlog_item(&mut board, make_backlog("b1")).unwrap();
    promote_to_active(&mut board, "b1", 1, None).unwrap();
    assert!(board.backlog.is_empty());
    assert_eq!(board.active.len(), 1);
    assert_eq!(board.active[0].id, "b1");
    assert!(matches!(board.active[0].status, GoalProgress::NotStarted));
}

#[test]
fn promote_to_active_not_found() {
    let mut board = GoalBoard::new();
    assert!(promote_to_active(&mut board, "nonexistent", 1, None).is_err());
}

#[test]
fn update_goal_progress_and_archive_completed() {
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, make_goal("g1", 1)).unwrap();
    add_active_goal(&mut board, make_goal("g2", 2)).unwrap();
    update_goal_progress(&mut board, "g1", GoalProgress::Completed).unwrap();
    let archived = archive_completed(&mut board);
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].id, "g1");
    assert_eq!(board.active.len(), 1);
}

#[test]
fn update_goal_progress_rejects_over_100_percent() {
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, make_goal("g1", 1)).unwrap();
    let result = update_goal_progress(&mut board, "g1", GoalProgress::InProgress { percent: 101 });
    assert!(result.is_err());
}

#[test]
fn seed_default_board_populates_empty_board() {
    let mut board = GoalBoard::new();
    let count = seed_default_board(&mut board);
    assert_eq!(count, DEFAULT_SEED_GOALS.len());
    assert_eq!(board.active.len(), DEFAULT_SEED_GOALS.len());
}

#[test]
fn seed_default_board_skips_non_empty() {
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, make_goal("existing", 1)).unwrap();
    let count = seed_default_board(&mut board);
    assert_eq!(count, 0);
    assert_eq!(board.active.len(), 1);
}

// ── enqueue_stewardship_issue (issue #1167) ─────────────────────────

#[test]
fn enqueue_stewardship_issue_adds_backlog_row() {
    let mut board = GoalBoard::new();
    super::enqueue_stewardship_issue(
        &mut board,
        "rysweet/Simard",
        42,
        "https://github.com/rysweet/Simard/issues/42",
        "abcdef0123456789",
    )
    .unwrap();
    assert_eq!(board.backlog.len(), 1);
    let item = &board.backlog[0];
    assert_eq!(item.id, "stewardship-rysweet_Simard-42");
    assert_eq!(item.source, "stewardship:rysweet/Simard#42");
    assert!(item.description.contains("abcdef0123456789"));
    assert!(
        item.description
            .contains("https://github.com/rysweet/Simard/issues/42")
    );
    assert!(item.score > 0.0 && item.score <= 1.0);
}

#[test]
fn enqueue_stewardship_issue_is_idempotent_on_same_issue() {
    let mut board = GoalBoard::new();
    super::enqueue_stewardship_issue(
        &mut board,
        "rysweet/Simard",
        42,
        "https://github.com/rysweet/Simard/issues/42",
        "sig",
    )
    .unwrap();
    // Second call with same (repo, issue#) → no-op (returns Ok, backlog unchanged).
    super::enqueue_stewardship_issue(
        &mut board,
        "rysweet/Simard",
        42,
        "https://github.com/rysweet/Simard/issues/42",
        "sig",
    )
    .unwrap();
    assert_eq!(board.backlog.len(), 1, "must not duplicate stewardship row");
}

#[test]
fn enqueue_stewardship_issue_amplihack_repo() {
    let mut board = GoalBoard::new();
    super::enqueue_stewardship_issue(
        &mut board,
        "rysweet/amplihack",
        7,
        "https://github.com/rysweet/amplihack/issues/7",
        "deadbeef",
    )
    .unwrap();
    let item = &board.backlog[0];
    assert_eq!(item.id, "stewardship-rysweet_amplihack-7");
    assert_eq!(item.source, "stewardship:rysweet/amplihack#7");
}

// ── load_goal_board: disk-first three-tier fallback (issue #1574) ────────────

/// Serialize access to SIMARD_STATE_ROOT across parallel test threads.
/// Without this, concurrent set_var / remove_var calls race.
static ENV_MUTEX: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

/// Helper: build a minimal mock bridge. search_facts always returns empty.
fn mock_bridge_empty() -> crate::memory_bridge::CognitiveMemoryBridge {
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::memory_bridge::CognitiveMemoryBridge;
    use serde_json::json;
    let transport =
        InMemoryBridgeTransport::new("test-disk-first", |method, _params| match method {
            "memory.search_facts" => Ok(json!({"facts": []})),
            "memory.store_fact" => Ok(json!({"id": "sem_x"})),
            "memory.store_episode" => Ok(json!({"id": "epi_x"})),
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    CognitiveMemoryBridge::new(Box::new(transport))
}

/// Helper: build a bridge whose search_facts returns a single snapshot fact
/// with the correct CognitiveFact wire format (node_id, concept, content,
/// confidence, source_id, tags).
fn mock_bridge_with_snapshot(board_json: &str) -> crate::memory_bridge::CognitiveMemoryBridge {
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::memory_bridge::CognitiveMemoryBridge;
    use serde_json::json;
    let board_json = board_json.to_string();
    let transport =
        InMemoryBridgeTransport::new("test-mem-fallback", move |method, _params| match method {
            "memory.search_facts" => Ok(json!({
                "facts": [{
                    "node_id": "f1",
                    "concept": "goal-board:snapshot",
                    "content": board_json,
                    "confidence": 1.0,
                    "source_id": "goal-curator",
                    "tags": ["goal-board"]
                }]
            })),
            "memory.store_fact" => Ok(json!({"id": "sem_x"})),
            "memory.store_episode" => Ok(json!({"id": "epi_x"})),
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    CognitiveMemoryBridge::new(Box::new(transport))
}

/// Helper: build a bridge whose search_facts always returns an error (simulates
/// cognitive memory being unavailable — e.g., bridge subprocess crash).
fn mock_bridge_search_fails() -> crate::memory_bridge::CognitiveMemoryBridge {
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::memory_bridge::CognitiveMemoryBridge;
    let transport = InMemoryBridgeTransport::new("test-search-fails", |method, _params| {
        Err(crate::bridge::BridgeErrorPayload {
            code: -32000,
            message: format!("simulated bridge failure for method: {method}"),
        })
    });
    CognitiveMemoryBridge::new(Box::new(transport))
}

/// Helper: unique temp dir for each test (avoids cross-test state pollution).
fn tmp_state_root(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("simard-test-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run `f` with SIMARD_STATE_ROOT set to `root`, restoring it afterwards.
/// Uses ENV_MUTEX to prevent races between parallel tests.
fn with_state_root<F, R>(root: &std::path::Path, f: F) -> R
where
    F: FnOnce() -> R,
{
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    unsafe { std::env::set_var("SIMARD_STATE_ROOT", root) };
    let result = f();
    unsafe { std::env::remove_var("SIMARD_STATE_ROOT") };
    result
}

/// Tier 1: when goal_records.json exists and is valid, load_goal_board must
/// return the board from disk without touching cognitive memory.
#[test]
fn load_goal_board_tier1_reads_from_disk() {
    let root = tmp_state_root("tier1");
    let mut expected = GoalBoard::new();
    expected.active.push(ActiveGoal {
        id: "disk-goal".to_string(),
        description: "Loaded from disk".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });
    let json = serde_json::to_string_pretty(&expected).unwrap();
    std::fs::write(root.join("goal_records.json"), &json).unwrap();

    // Bridge returns empty — so if we get the goal it came from disk.
    let bridge = mock_bridge_empty();
    let board = with_state_root(&root, || super::load_goal_board(&bridge).unwrap());

    assert_eq!(board.active.len(), 1, "must load from disk (tier 1)");
    assert_eq!(board.active[0].id, "disk-goal");
}

/// Tier 2: when goal_records.json is absent, load_goal_board must fall back to
/// cognitive memory and return the snapshot stored there.
#[test]
fn load_goal_board_tier2_falls_back_to_cognitive_memory_when_no_disk_file() {
    let root = tmp_state_root("tier2-missing");
    // Do NOT create goal_records.json.

    let mut mem_board = GoalBoard::new();
    mem_board.active.push(ActiveGoal {
        id: "mem-goal".to_string(),
        description: "Loaded from memory".to_string(),
        priority: 2,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });
    let snapshot_json = serde_json::to_string(&mem_board).unwrap();
    let bridge = mock_bridge_with_snapshot(&snapshot_json);

    let board = with_state_root(&root, || super::load_goal_board(&bridge).unwrap());

    assert_eq!(
        board.active.len(),
        1,
        "must fall back to cognitive memory (tier 2)"
    );
    assert_eq!(board.active[0].id, "mem-goal");
}

/// Tier 2 (parse error path): when goal_records.json contains invalid JSON,
/// load_goal_board must fall back to cognitive memory rather than failing.
#[test]
fn load_goal_board_tier2_falls_back_on_corrupt_disk_file() {
    let root = tmp_state_root("tier2-corrupt");
    std::fs::write(root.join("goal_records.json"), b"THIS IS NOT JSON").unwrap();

    let mut mem_board = GoalBoard::new();
    mem_board.active.push(ActiveGoal {
        id: "recover-goal".to_string(),
        description: "Recovered from memory after disk corruption".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });
    let snapshot_json = serde_json::to_string(&mem_board).unwrap();
    let bridge = mock_bridge_with_snapshot(&snapshot_json);

    let board = with_state_root(&root, || super::load_goal_board(&bridge).unwrap());

    assert_eq!(
        board.active.len(),
        1,
        "must recover from memory (tier 2) after disk corruption"
    );
    assert_eq!(board.active[0].id, "recover-goal");
}

/// Tier 3: when both disk and cognitive memory are absent, load_goal_board
/// must return an empty board (not an error).
#[test]
fn load_goal_board_tier3_returns_empty_board_when_all_sources_absent() {
    let root = tmp_state_root("tier3");
    // No disk file, bridge returns empty facts.
    let bridge = mock_bridge_empty();

    let board = with_state_root(&root, || super::load_goal_board(&bridge).unwrap());

    assert!(board.active.is_empty(), "must return empty board (tier 3)");
    assert!(board.backlog.is_empty());
}

/// When cognitive memory search_facts returns an error (e.g., bridge crashed),
/// load_goal_board must fall through to Tier 3 (empty board) rather than
/// propagating the error.  Resilience: bridge unavailability ≠ fatal.
#[test]
fn load_goal_board_tier2_bridge_error_falls_through_to_empty_board() {
    let root = tmp_state_root("tier2-err");
    // No disk file; bridge returns an error from search_facts.
    let bridge = mock_bridge_search_fails();

    let board = with_state_root(&root, || super::load_goal_board(&bridge).unwrap());

    assert!(
        board.active.is_empty(),
        "bridge search_facts error must degrade to empty board (tier 3), not propagate"
    );
    assert!(board.backlog.is_empty());
}

/// Disk file wins over cognitive memory even when memory also has a snapshot.
/// (Verifies tier ordering — disk is always the primary source of truth.)
#[test]
fn load_goal_board_disk_beats_cognitive_memory() {
    let root = tmp_state_root("tier1-priority");

    let mut disk_board = GoalBoard::new();
    disk_board.active.push(ActiveGoal {
        id: "disk-wins".to_string(),
        description: "From disk".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });
    std::fs::write(
        root.join("goal_records.json"),
        serde_json::to_string_pretty(&disk_board).unwrap(),
    )
    .unwrap();

    // Memory has a *different* goal — if disk is not preferred this test fails.
    let mut mem_board = GoalBoard::new();
    mem_board.active.push(ActiveGoal {
        id: "mem-stale".to_string(),
        description: "Stale cognitive memory snapshot".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });
    let snapshot_json = serde_json::to_string(&mem_board).unwrap();
    let bridge = mock_bridge_with_snapshot(&snapshot_json);

    let board = with_state_root(&root, || super::load_goal_board(&bridge).unwrap());

    assert_eq!(
        board.active[0].id, "disk-wins",
        "disk must beat cognitive memory"
    );
}

/// save_goal_board must write goal_records.json to disk in addition to calling
/// the cognitive memory bridge.
#[test]
fn save_goal_board_writes_to_disk() {
    let root = tmp_state_root("save-disk");
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        id: "saved-goal".to_string(),
        description: "Written to disk".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });

    let bridge = mock_bridge_empty();
    with_state_root(&root, || super::save_goal_board(&board, &bridge).unwrap());

    let disk_path = root.join("goal_records.json");
    assert!(
        disk_path.exists(),
        "goal_records.json must be created by save_goal_board"
    );

    let content = std::fs::read_to_string(&disk_path).unwrap();
    let reloaded: GoalBoard = serde_json::from_str(&content).unwrap();
    assert_eq!(reloaded.active.len(), 1);
    assert_eq!(reloaded.active[0].id, "saved-goal");
}

/// clear_goal_assignment must set assigned_to = None and reset status to
/// NotStarted regardless of the previous progress state.
#[test]
fn clear_goal_assignment_resets_status_and_clears_assigned_to() {
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        id: "assigned-goal".to_string(),
        description: "Has an engineer".to_string(),
        priority: 1,
        status: GoalProgress::InProgress { percent: 30 },
        assigned_to: Some("engineer-session-abc".to_string()),
        current_activity: Some("Doing work".to_string()),
        wip_refs: vec![],
    });

    super::clear_goal_assignment(&mut board, "assigned-goal").unwrap();

    let goal = &board.active[0];
    assert!(
        goal.assigned_to.is_none(),
        "assigned_to must be cleared after clear_goal_assignment"
    );
    assert!(
        matches!(goal.status, GoalProgress::NotStarted),
        "status must be NotStarted after clear_goal_assignment, got {:?}",
        goal.status
    );
}

/// clear_goal_assignment on a missing goal returns Err.
#[test]
fn clear_goal_assignment_returns_err_for_missing_goal() {
    let mut board = GoalBoard::new();
    let err = super::clear_goal_assignment(&mut board, "nonexistent").unwrap_err();
    assert!(err.to_string().contains("not found"), "{err}");
}
