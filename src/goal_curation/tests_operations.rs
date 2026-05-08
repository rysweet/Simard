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

// ── load_goal_board / save_goal_board: memory-only contract (issue #1590) ──

/// Serialize access to SIMARD_STATE_ROOT across parallel test threads.
/// Without this, concurrent set_var / remove_var calls race.
static ENV_MUTEX: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

/// Record of a `memory.store_fact` call captured by the in-memory bridge.
#[derive(Clone, Debug)]
struct StoredFactCall {
    concept: String,
    content: String,
}

/// Shared mutable state captured by the in-memory bridge handler closure.
#[derive(Default)]
struct BridgeRecording {
    stored_facts: std::sync::Mutex<Vec<StoredFactCall>>,
}

impl BridgeRecording {
    fn shared() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self::default())
    }

    fn calls(&self) -> Vec<StoredFactCall> {
        self.stored_facts.lock().unwrap().clone()
    }
}

/// Build a recording bridge whose `memory.search_facts` returns no facts and
/// whose `memory.store_fact` records every call into the supplied recording.
fn recording_bridge_empty(
    recording: std::sync::Arc<BridgeRecording>,
) -> crate::memory_bridge::CognitiveMemoryBridge {
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::memory_bridge::CognitiveMemoryBridge;
    use serde_json::json;
    let recording_for_handler = recording;
    let transport =
        InMemoryBridgeTransport::new("test-record-empty", move |method, params| match method {
            "memory.search_facts" => Ok(json!({"facts": []})),
            "memory.store_fact" => {
                let concept = params
                    .get("concept")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let content = params
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                recording_for_handler
                    .stored_facts
                    .lock()
                    .unwrap()
                    .push(StoredFactCall { concept, content });
                Ok(json!({"id": "sem_x"}))
            }
            "memory.store_episode" => Ok(json!({"id": "epi_x"})),
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    CognitiveMemoryBridge::new(Box::new(transport))
}

/// Build a recording bridge whose `memory.search_facts` returns the given
/// snapshot fact and whose `memory.store_fact` records every call.
fn recording_bridge_with_snapshot(
    snapshot_json: &str,
    recording: std::sync::Arc<BridgeRecording>,
) -> crate::memory_bridge::CognitiveMemoryBridge {
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::memory_bridge::CognitiveMemoryBridge;
    use serde_json::json;
    let snapshot_json = snapshot_json.to_string();
    let recording_for_handler = recording;
    let transport =
        InMemoryBridgeTransport::new("test-record-snapshot", move |method, params| match method {
            "memory.search_facts" => Ok(json!({
                "facts": [{
                    "node_id": "f1",
                    "concept": "goal-board:snapshot",
                    "content": snapshot_json,
                    "confidence": 1.0,
                    "source_id": "goal-curator",
                    "tags": ["goal-board"]
                }]
            })),
            "memory.store_fact" => {
                let concept = params
                    .get("concept")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let content = params
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                recording_for_handler
                    .stored_facts
                    .lock()
                    .unwrap()
                    .push(StoredFactCall { concept, content });
                Ok(json!({"id": "sem_x"}))
            }
            "memory.store_episode" => Ok(json!({"id": "epi_x"})),
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    CognitiveMemoryBridge::new(Box::new(transport))
}

/// Build a bridge whose `memory.search_facts` always returns an error
/// (simulates the cognitive-memory subprocess being unavailable).
fn bridge_search_fails() -> crate::memory_bridge::CognitiveMemoryBridge {
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
    let dir = std::env::temp_dir().join(format!(
        "simard-test-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
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
    // SAFETY: serialised by ENV_MUTEX; no other threads observe this var.
    unsafe { std::env::set_var("SIMARD_STATE_ROOT", root) };
    let result = f();
    // SAFETY: same as above.
    unsafe { std::env::remove_var("SIMARD_STATE_ROOT") };
    result
}

#[test]
fn load_goal_board_reads_from_cognitive_memory() {
    let root = tmp_state_root("mem-read");
    let mut mem_board = GoalBoard::new();
    mem_board.active.push(ActiveGoal {
        id: "memory-only-goal".to_string(),
        description: "Loaded straight from cognitive memory".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });
    let snapshot_json = serde_json::to_string(&mem_board).unwrap();
    let recording = BridgeRecording::shared();
    let bridge = recording_bridge_with_snapshot(&snapshot_json, recording.clone());

    let board = with_state_root(&root, || super::load_goal_board(&bridge).unwrap());

    assert_eq!(board.active.len(), 1);
    assert_eq!(board.active[0].id, "memory-only-goal");
    assert!(
        recording.calls().is_empty(),
        "load_goal_board with no legacy file must not call store_fact"
    );
}

#[test]
fn load_goal_board_returns_empty_when_memory_has_no_snapshot() {
    let root = tmp_state_root("mem-empty");
    let recording = BridgeRecording::shared();
    let bridge = recording_bridge_empty(recording.clone());

    let board = with_state_root(&root, || super::load_goal_board(&bridge).unwrap());

    assert!(board.active.is_empty());
    assert!(board.backlog.is_empty());
    assert!(recording.calls().is_empty());
}

#[test]
fn load_goal_board_returns_empty_when_search_facts_errors() {
    let root = tmp_state_root("mem-err");
    let bridge = bridge_search_fails();

    let board = with_state_root(&root, || super::load_goal_board(&bridge).unwrap());

    assert!(board.active.is_empty());
    assert!(board.backlog.is_empty());
}

#[test]
fn load_goal_board_migrates_legacy_disk_file_into_memory_then_deletes_it() {
    let root = tmp_state_root("migrate");
    let mut legacy = GoalBoard::new();
    legacy.active.push(ActiveGoal {
        id: "legacy-goal".to_string(),
        description: "Originally on disk".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });
    let path = root.join("goal_records.json");
    std::fs::write(&path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

    let recording = BridgeRecording::shared();
    let bridge = recording_bridge_empty(recording.clone());
    let _ = with_state_root(&root, || super::load_goal_board(&bridge).unwrap());

    assert!(
        !path.exists(),
        "legacy file must be deleted after migration"
    );
    let calls = recording.calls();
    assert_eq!(calls.len(), 1, "exactly one store_fact call expected");
    assert_eq!(calls[0].concept, "goal-board:snapshot");
    let migrated: GoalBoard = serde_json::from_str(&calls[0].content).unwrap();
    assert_eq!(migrated.active.len(), 1);
    assert_eq!(migrated.active[0].id, "legacy-goal");
}

#[test]
fn load_goal_board_migration_is_noop_when_no_legacy_file() {
    let root = tmp_state_root("migrate-noop");
    let recording = BridgeRecording::shared();
    let bridge = recording_bridge_empty(recording.clone());

    let _ = with_state_root(&root, || super::load_goal_board(&bridge).unwrap());

    assert!(
        recording.calls().is_empty(),
        "no migration write expected when legacy file is absent"
    );
}

#[test]
fn load_goal_board_migration_handles_corrupt_legacy_file_without_panic() {
    let root = tmp_state_root("migrate-corrupt");
    let path = root.join("goal_records.json");
    std::fs::write(&path, b"NOT VALID JSON").unwrap();

    let recording = BridgeRecording::shared();
    let bridge = recording_bridge_empty(recording.clone());
    let board = with_state_root(&root, || super::load_goal_board(&bridge).unwrap());

    assert!(board.active.is_empty(), "must return empty board");
    assert!(
        recording.calls().is_empty(),
        "corrupt file must not be migrated"
    );
    assert!(
        path.exists(),
        "corrupt legacy file must be left on disk for operator inspection"
    );
}

#[test]
fn load_goal_board_runs_migration_only_once_in_practice() {
    // After the first call, the file is gone, so the second call's migration
    // is a no-op (gated on path.exists()).  We assert:
    //   call 1: 1 store_fact (the migration), file deleted
    //   call 2: 0 additional store_facts, file still absent
    let root = tmp_state_root("migrate-once");
    let mut legacy = GoalBoard::new();
    legacy.active.push(ActiveGoal {
        id: "once-goal".to_string(),
        description: "Migrate exactly once".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });
    let path = root.join("goal_records.json");
    std::fs::write(&path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

    let recording = BridgeRecording::shared();
    let bridge = recording_bridge_empty(recording.clone());
    with_state_root(&root, || {
        super::load_goal_board(&bridge).unwrap();
        super::load_goal_board(&bridge).unwrap();
    });

    assert!(!path.exists());
    assert_eq!(
        recording.calls().len(),
        1,
        "migration must only run once across repeated load_goal_board calls"
    );
}

#[test]
fn save_goal_board_persists_only_to_memory_and_writes_no_disk_file() {
    let root = tmp_state_root("save-mem-only");
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        id: "memory-saved-goal".to_string(),
        description: "Persisted only to memory".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });

    let recording = BridgeRecording::shared();
    let bridge = recording_bridge_empty(recording.clone());
    with_state_root(&root, || super::save_goal_board(&board, &bridge).unwrap());

    assert!(
        !root.join("goal_records.json").exists(),
        "save_goal_board must not write goal_records.json to disk"
    );
    let calls = recording.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].concept, "goal-board:snapshot");
    let persisted: GoalBoard = serde_json::from_str(&calls[0].content).unwrap();
    assert_eq!(persisted.active.len(), 1);
    assert_eq!(persisted.active[0].id, "memory-saved-goal");
}

#[test]
fn save_goal_board_rejects_suspect_board_without_persisting() {
    // Suspect by short id: "g1" is < 5 chars → board_integrity_suspect fires.
    let root = tmp_state_root("save-suspect");
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        id: "g1".to_string(),
        description: "Goal g1".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });

    let recording = BridgeRecording::shared();
    let bridge = recording_bridge_empty(recording.clone());
    let err = with_state_root(&root, || {
        super::save_goal_board(&board, &bridge).unwrap_err()
    });

    assert!(
        err.to_string().contains("suspect"),
        "expected 'suspect' in error: {err}"
    );
    assert!(
        recording.calls().is_empty(),
        "store_fact must not be called for a suspect board"
    );
    assert!(
        !root.join("goal_records.json").exists(),
        "no disk file must be written for a suspect board"
    );
}

#[test]
fn save_goal_board_accepts_a_well_formed_board() {
    let root = tmp_state_root("save-ok");
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        id: "well-formed-goal".to_string(),
        description: "Improve test coverage on the goal curation module".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });
    let recording = BridgeRecording::shared();
    let bridge = recording_bridge_empty(recording.clone());

    with_state_root(&root, || super::save_goal_board(&board, &bridge).unwrap());

    assert_eq!(recording.calls().len(), 1);
}

// ── board_integrity_suspect / is_placeholder_description (ported from cycle.rs) ──

#[test]
fn is_placeholder_description_matches_short_lowercase_goal_phrase() {
    assert!(is_placeholder_description("Goal g1"));
    assert!(is_placeholder_description("goal g1"));
    assert!(is_placeholder_description("GOAL abc"));
    assert!(is_placeholder_description("  goal g1  "));
}

#[test]
fn is_placeholder_description_rejects_long_or_substantive_descriptions() {
    assert!(!is_placeholder_description("Ship the v1 release"));
    assert!(!is_placeholder_description("goal g12345"));
    assert!(!is_placeholder_description(""));
}

#[test]
fn board_integrity_suspect_flags_short_ids_and_placeholder_descriptions() {
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        id: "g1".to_string(),
        description: "Goal g1".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });
    assert!(board_integrity_suspect(&board).is_some());
}

#[test]
fn board_integrity_suspect_passes_well_formed_board() {
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        id: "improve-amplihack-test-coverage".to_string(),
        description: "Increase test coverage across the amplihack ecosystem".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });
    assert!(board_integrity_suspect(&board).is_none());
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
