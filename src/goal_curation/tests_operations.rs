use super::operations::*;
use super::types::{ActiveGoal, BacklogItem, GoalBoard, GoalProgress, MAX_ACTIVE_GOALS};

fn make_goal(id: &str, priority: u32) -> ActiveGoal {
    ActiveGoal {
        labels: Vec::new(),
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
        id: id.to_string(),
        description: format!("Goal {id}"),
        priority,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
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
fn promote_to_active_is_fail_closed_and_leaves_backlog_untouched() {
    // Issue #4930 (B2): `promote_to_active` must run the SAME admission gate as
    // the direct-add path, not just `validate_priority`. A backlog item whose
    // record would produce an invalid active goal (here: an empty description,
    // as a corrupt persisted board could carry) must be rejected — and because
    // validation runs BEFORE the board is mutated, the item stays in the backlog
    // rather than being silently dropped.
    let mut board = GoalBoard::new();
    board.backlog.push(BacklogItem {
        id: "b-invalid".to_string(),
        description: String::new(),
        source: "test".to_string(),
        score: 0.0,
    });
    let err = promote_to_active(&mut board, "b-invalid", 1, None);
    assert!(
        err.is_err(),
        "an item that would yield an invalid active goal must be rejected on promotion"
    );
    assert_eq!(
        board.backlog.len(),
        1,
        "a rejected promotion must leave the backlog item in place (no silent loss)"
    );
    assert!(
        board.active.is_empty(),
        "a rejected promotion must not insert into the active board"
    );
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

// ── load_goal_board / save_goal_board: memory-only contract (issue #1590) ──

/// Serialize access to SIMARD_STATE_ROOT across parallel test threads.
/// Without this, concurrent set_var / remove_var calls race.
static ENV_MUTEX: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

/// Record of a `memory.store_fact` call captured by the in-memory memory.
#[derive(Clone, Debug)]
struct StoredFactCall {
    concept: String,
    content: String,
}

/// Shared mutable state captured by the in-memory memory handler closure.
#[derive(Default)]
struct RpcRecording {
    stored_facts: std::sync::Mutex<Vec<StoredFactCall>>,
}

impl RpcRecording {
    fn shared() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self::default())
    }

    fn calls(&self) -> Vec<StoredFactCall> {
        self.stored_facts.lock().unwrap().clone()
    }
}

/// Build a recording memory whose `memory.search_facts` returns no facts and
/// whose `memory.store_fact` records every call into the supplied recording.
fn recording_memory_empty(
    recording: std::sync::Arc<RpcRecording>,
) -> crate::memory_client::CognitiveMemoryClient {
    use crate::memory_client::CognitiveMemoryClient;
    use crate::rpc_transport::InMemoryRpcTransport;
    use serde_json::json;
    let recording_for_handler = recording;
    let transport =
        InMemoryRpcTransport::new("test-record-empty", move |method, params| match method {
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
            _ => Err(crate::rpc::RpcErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    CognitiveMemoryClient::new(Box::new(transport))
}

/// Build a recording memory whose `memory.search_facts` returns the given
/// snapshot fact and whose `memory.store_fact` records every call.
fn recording_memory_with_snapshot(
    snapshot_json: &str,
    recording: std::sync::Arc<RpcRecording>,
) -> crate::memory_client::CognitiveMemoryClient {
    use crate::memory_client::CognitiveMemoryClient;
    use crate::rpc_transport::InMemoryRpcTransport;
    use serde_json::json;
    let snapshot_json = snapshot_json.to_string();
    let recording_for_handler = recording;
    let transport =
        InMemoryRpcTransport::new("test-record-snapshot", move |method, params| match method {
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
            _ => Err(crate::rpc::RpcErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    CognitiveMemoryClient::new(Box::new(transport))
}

/// Build a memory whose `memory.search_facts` always returns an error
/// (simulates the cognitive-memory subprocess being unavailable).
fn memory_search_fails() -> crate::memory_client::CognitiveMemoryClient {
    use crate::memory_client::CognitiveMemoryClient;
    use crate::rpc_transport::InMemoryRpcTransport;
    let transport = InMemoryRpcTransport::new("test-search-fails", |method, _params| {
        Err(crate::rpc::RpcErrorPayload {
            code: -32000,
            message: format!("simulated memory failure for method: {method}"),
        })
    });
    CognitiveMemoryClient::new(Box::new(transport))
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
/// Uses ENV_MUTEX for intra-module ordering; callers are additionally
/// `#[serial(cognitive_memory)]` so no test in another module reads
/// SIMARD_STATE_ROOT while it is pinned here (issue #2316).
fn with_state_root<F, R>(root: &std::path::Path, f: F) -> R
where
    F: FnOnce() -> R,
{
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: serialised by ENV_MUTEX within this module and by the
    // `cognitive_memory` serial key across modules.
    unsafe { std::env::set_var("SIMARD_STATE_ROOT", root) };
    let result = f();
    // SAFETY: same as above.
    unsafe { std::env::remove_var("SIMARD_STATE_ROOT") };
    result
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn load_goal_board_reads_from_cognitive_memory() {
    let root = tmp_state_root("mem-read");
    let mut mem_board = GoalBoard::new();
    mem_board.active.push(ActiveGoal {
        labels: Vec::new(),
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
        id: "memory-only-goal".to_string(),
        description: "Loaded straight from cognitive memory".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    });
    let snapshot_json = serde_json::to_string(&mem_board).unwrap();
    let recording = RpcRecording::shared();
    let memory = recording_memory_with_snapshot(&snapshot_json, recording.clone());

    let board = with_state_root(&root, || super::load_goal_board(&memory).unwrap());

    assert_eq!(board.active.len(), 1);
    assert_eq!(board.active[0].id, "memory-only-goal");
    assert!(
        recording.calls().is_empty(),
        "load_goal_board with no legacy file must not call store_fact"
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn load_goal_board_returns_empty_when_memory_has_no_snapshot() {
    let root = tmp_state_root("mem-empty");
    let recording = RpcRecording::shared();
    let memory = recording_memory_empty(recording.clone());

    let board = with_state_root(&root, || super::load_goal_board(&memory).unwrap());

    assert!(board.active.is_empty());
    assert!(board.backlog.is_empty());
    assert!(recording.calls().is_empty());
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn load_goal_board_returns_empty_when_search_facts_errors() {
    let root = tmp_state_root("mem-err");
    let memory = memory_search_fails();

    let board = with_state_root(&root, || super::load_goal_board(&memory).unwrap());

    assert!(board.active.is_empty());
    assert!(board.backlog.is_empty());
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn load_goal_board_migrates_legacy_disk_file_into_memory_then_deletes_it() {
    let root = tmp_state_root("migrate");
    let mut legacy = GoalBoard::new();
    legacy.active.push(ActiveGoal {
        labels: Vec::new(),
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
        id: "legacy-goal".to_string(),
        description: "Originally on disk".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    });
    let path = root.join("goal_records.json");
    std::fs::write(&path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

    let recording = RpcRecording::shared();
    let memory = recording_memory_empty(recording.clone());
    let _ = with_state_root(&root, || super::load_goal_board(&memory).unwrap());

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
#[serial_test::serial(cognitive_memory)]
fn load_goal_board_migration_is_noop_when_no_legacy_file() {
    let root = tmp_state_root("migrate-noop");
    let recording = RpcRecording::shared();
    let memory = recording_memory_empty(recording.clone());

    let _ = with_state_root(&root, || super::load_goal_board(&memory).unwrap());

    assert!(
        recording.calls().is_empty(),
        "no migration write expected when legacy file is absent"
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn load_goal_board_migration_handles_corrupt_legacy_file_without_panic() {
    let root = tmp_state_root("migrate-corrupt");
    let path = root.join("goal_records.json");
    std::fs::write(&path, b"NOT VALID JSON").unwrap();

    let recording = RpcRecording::shared();
    let memory = recording_memory_empty(recording.clone());
    let board = with_state_root(&root, || super::load_goal_board(&memory).unwrap());

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
#[serial_test::serial(cognitive_memory)]
fn load_goal_board_runs_migration_only_once_in_practice() {
    // After the first call, the file is gone, so the second call's migration
    // is a no-op (gated on path.exists()).  We assert:
    //   call 1: 1 store_fact (the migration), file deleted
    //   call 2: 0 additional store_facts, file still absent
    let root = tmp_state_root("migrate-once");
    let mut legacy = GoalBoard::new();
    legacy.active.push(ActiveGoal {
        labels: Vec::new(),
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
        id: "once-goal".to_string(),
        description: "Migrate exactly once".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    });
    let path = root.join("goal_records.json");
    std::fs::write(&path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

    let recording = RpcRecording::shared();
    let memory = recording_memory_empty(recording.clone());
    with_state_root(&root, || {
        super::load_goal_board(&memory).unwrap();
        super::load_goal_board(&memory).unwrap();
    });

    assert!(!path.exists());
    assert_eq!(
        recording.calls().len(),
        1,
        "migration must only run once across repeated load_goal_board calls"
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn save_goal_board_persists_only_to_memory_and_writes_no_disk_file() {
    let root = tmp_state_root("save-mem-only");
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        labels: Vec::new(),
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
        id: "memory-saved-goal".to_string(),
        description: "Persisted only to memory".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    });

    let recording = RpcRecording::shared();
    let memory = recording_memory_empty(recording.clone());
    with_state_root(&root, || super::save_goal_board(&board, &memory).unwrap());

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
#[serial_test::serial(cognitive_memory)]
fn save_goal_board_rejects_suspect_board_without_persisting() {
    // Suspect by short id: "g1" is < 5 chars → board_integrity_suspect fires.
    let root = tmp_state_root("save-suspect");
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        labels: Vec::new(),
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
        id: "g1".to_string(),
        description: "Goal g1".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    });

    let recording = RpcRecording::shared();
    let memory = recording_memory_empty(recording.clone());
    let err = with_state_root(&root, || {
        super::save_goal_board(&board, &memory).unwrap_err()
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
#[serial_test::serial(cognitive_memory)]
fn save_goal_board_accepts_a_well_formed_board() {
    let root = tmp_state_root("save-ok");
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        labels: Vec::new(),
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
        id: "well-formed-goal".to_string(),
        description: "Improve test coverage on the goal curation module".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    });
    let recording = RpcRecording::shared();
    let memory = recording_memory_empty(recording.clone());

    with_state_root(&root, || super::save_goal_board(&board, &memory).unwrap());

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
        labels: Vec::new(),
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
        id: "g1".to_string(),
        description: "Goal g1".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    });
    assert!(board_integrity_suspect(&board).is_some());
}

#[test]
fn board_integrity_suspect_passes_well_formed_board() {
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        labels: Vec::new(),
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
        id: "improve-amplihack-test-coverage".to_string(),
        description: "Increase test coverage across the amplihack ecosystem".to_string(),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    });
    assert!(board_integrity_suspect(&board).is_none());
}

/// clear_goal_assignment must set assigned_to = None and reset status to
/// NotStarted regardless of the previous progress state.
#[test]
fn clear_goal_assignment_resets_status_and_clears_assigned_to() {
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal {
        labels: Vec::new(),
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
        id: "assigned-goal".to_string(),
        description: "Has an engineer".to_string(),
        priority: 1,
        status: GoalProgress::InProgress { percent: 30 },
        assigned_to: Some("engineer-session-abc".to_string()),
        current_activity: Some("Doing work".to_string()),
        wip_refs: vec![],
        last_progress_update_at: None,
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

// ═════════════════════════════════════════════════════════════════════════
// Issue #1915 — merge-on-write semantics for save_goal_board
// ═════════════════════════════════════════════════════════════════════════
//
// These tests specify the merge-on-write contract introduced to prevent
// concurrent CognitiveMemoryOps clients from silently clobbering each
// other's goals (root cause of #1915). They reference three new symbols:
//
//   - `merge_boards(persisted, in_flight) -> GoalBoard`     (pure helper)
//   - `read_latest_snapshot(memory) -> Option<GoalBoard>`   (read helper)
//   - revised `save_goal_board(...)` that performs
//        guard(in_flight) -> read_latest_snapshot -> merge -> store_fact
//
// All tests in this section MUST fail against the pre-fix code and pass
// after the fix is applied.

use super::operations::{merge_boards, read_latest_snapshot};

// ── shared test helpers ─────────────────────────────────────────────────

/// Build an `ActiveGoal` with id/priority/status/description set.
/// All other fields default to `None` / `vec![]`.
fn goal_with(id: &str, priority: u32, status: GoalProgress, desc: &str) -> ActiveGoal {
    ActiveGoal {
        labels: Vec::new(),
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
        id: id.to_string(),
        description: desc.to_string(),
        priority,
        status,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    }
}

/// Build a `BacklogItem` with id/description/source/score set.
fn backlog_with(id: &str, description: &str, source: &str, score: f64) -> BacklogItem {
    BacklogItem {
        id: id.to_string(),
        description: description.to_string(),
        source: source.to_string(),
        score,
    }
}

/// Stateful in-memory memory that simulates the LadybugDB append-only fact
/// store. Every `store_fact` appends to a shared `Vec<CognitiveFact>` with a
/// monotonically-increasing `node_id` (so `max_by(node_id)` always picks
/// the most recent snapshot — matching production uuid-v7 semantics).
/// Every `search_facts` returns the current shared vec.
///
/// Returns the memory and a handle to the shared facts vec so tests can
/// assert on stored counts/content.
#[allow(clippy::type_complexity)]
fn stateful_memory() -> (
    crate::memory_client::CognitiveMemoryClient,
    std::sync::Arc<std::sync::Mutex<Vec<crate::memory_cognitive::CognitiveFact>>>,
) {
    use crate::memory_client::CognitiveMemoryClient;
    use crate::memory_cognitive::CognitiveFact;
    use crate::rpc_transport::InMemoryRpcTransport;
    use serde_json::json;

    let facts: std::sync::Arc<std::sync::Mutex<Vec<CognitiveFact>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let facts_for_handler = facts.clone();
    let counter_for_handler = counter.clone();

    let transport = InMemoryRpcTransport::new("test-stateful", move |method, params| {
        match method {
            "memory.search_facts" => {
                let snapshot = facts_for_handler.lock().unwrap().clone();
                let serialized = serde_json::to_value(&snapshot).unwrap();
                Ok(json!({ "facts": serialized }))
            }
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
                let confidence = params
                    .get("confidence")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.0);
                let source_id = params
                    .get("source_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let tags: Vec<String> = params
                    .get("tags")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let n = counter_for_handler.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                // Zero-padded so lexicographic max == numeric max.
                let node_id = format!("fact-{n:020}");
                facts_for_handler.lock().unwrap().push(CognitiveFact {
                    node_id: node_id.clone(),
                    concept,
                    content,
                    confidence,
                    source_id,
                    tags,
                    usage_count: 0,
                    last_accessed_at: None,
                });
                Ok(json!({ "id": node_id }))
            }
            "memory.store_episode" => Ok(json!({ "id": "epi_x" })),
            _ => Err(crate::rpc::RpcErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        }
    });
    (CognitiveMemoryClient::new(Box::new(transport)), facts)
}

/// Client whose `memory.search_facts` always errors but whose
/// `memory.store_fact` succeeds and is recorded. Used to verify the
/// read-failure fallback path in `save_goal_board`.
fn memory_search_fails_store_works(
    recording: std::sync::Arc<RpcRecording>,
) -> crate::memory_client::CognitiveMemoryClient {
    use crate::memory_client::CognitiveMemoryClient;
    use crate::rpc_transport::InMemoryRpcTransport;
    use serde_json::json;
    let recording_for_handler = recording;
    let transport =
        InMemoryRpcTransport::new("test-search-fails-store-works", move |method, params| {
            match method {
                "memory.search_facts" => Err(crate::rpc::RpcErrorPayload {
                    code: -32000,
                    message: "simulated search_facts failure".to_string(),
                }),
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
                    Ok(json!({ "id": "sem_x" }))
                }
                "memory.store_episode" => Ok(json!({ "id": "epi_x" })),
                _ => Err(crate::rpc::RpcErrorPayload {
                    code: -32601,
                    message: format!("unknown method: {method}"),
                }),
            }
        });
    CognitiveMemoryClient::new(Box::new(transport))
}

// ── merge_boards: pure helper unit tests (9 cases per design spec) ──────

/// (a) Disjoint active ids → union of both.
#[test]
fn merge_boards_disjoint_active_ids_unions_both() {
    let persisted = GoalBoard {
        active: vec![goal_with(
            "alpha-goal-aaaa",
            1,
            GoalProgress::NotStarted,
            "Alpha",
        )],
        backlog: vec![],
    };
    let in_flight = GoalBoard {
        active: vec![goal_with(
            "beta-goal-bbbb",
            2,
            GoalProgress::NotStarted,
            "Beta",
        )],
        backlog: vec![],
    };
    let merged = merge_boards(persisted, in_flight);
    assert_eq!(merged.active.len(), 2);
    let ids: Vec<&str> = merged.active.iter().map(|g| g.id.as_str()).collect();
    assert!(ids.contains(&"alpha-goal-aaaa"));
    assert!(ids.contains(&"beta-goal-bbbb"));
}

/// (b) Active collision on id → in-flight wins for ALL fields
/// (description, priority, status, assigned_to, current_activity).
#[test]
fn merge_boards_active_collision_in_flight_wins_all_fields() {
    let persisted = GoalBoard {
        active: vec![goal_with(
            "shared-id-xxxx",
            5,
            GoalProgress::NotStarted,
            "Persisted desc",
        )],
        backlog: vec![],
    };
    let mut in_flight_goal = goal_with(
        "shared-id-xxxx",
        1,
        GoalProgress::InProgress { percent: 75 },
        "In-flight desc",
    );
    in_flight_goal.assigned_to = Some("engineer-1".to_string());
    in_flight_goal.current_activity = Some("compiling".to_string());
    let in_flight = GoalBoard {
        active: vec![in_flight_goal],
        backlog: vec![],
    };
    let merged = merge_boards(persisted, in_flight);
    assert_eq!(merged.active.len(), 1);
    let g = &merged.active[0];
    assert_eq!(
        g.description, "In-flight desc",
        "description must come from in-flight"
    );
    assert_eq!(g.priority, 1, "priority must come from in-flight");
    assert!(
        matches!(g.status, GoalProgress::InProgress { percent: 75 }),
        "status must come from in-flight, got {:?}",
        g.status
    );
    assert_eq!(g.assigned_to.as_deref(), Some("engineer-1"));
    assert_eq!(g.current_activity.as_deref(), Some("compiling"));
}

/// (c) Backlog union by id with in-flight precedence on collision.
#[test]
fn merge_boards_backlog_unions_by_id_with_in_flight_precedence() {
    let persisted = GoalBoard {
        active: vec![],
        backlog: vec![
            backlog_with("b-only-persisted", "Only persisted", "p", 0.1),
            backlog_with("b-shared", "Old persisted desc", "p", 0.2),
        ],
    };
    let in_flight = GoalBoard {
        active: vec![],
        backlog: vec![
            backlog_with("b-only-flight", "Only flight", "f", 0.5),
            backlog_with("b-shared", "Newer flight desc", "f", 0.9),
        ],
    };
    let merged = merge_boards(persisted, in_flight);
    assert_eq!(
        merged.backlog.len(),
        3,
        "union: only-persisted + only-flight + shared = 3"
    );
    let shared = merged
        .backlog
        .iter()
        .find(|b| b.id == "b-shared")
        .expect("shared backlog id must be present");
    assert_eq!(shared.description, "Newer flight desc");
    assert_eq!(shared.source, "f");
    assert!((shared.score - 0.9).abs() < f64::EPSILON);
}

/// (d) Active overflow (>MAX_ACTIVE_GOALS) → truncate by priority ASC
/// (lowest priority number = highest importance kept).
#[test]
fn merge_boards_active_overflow_truncates_to_max_keeping_lowest_priority() {
    // Build a union of exactly MAX_ACTIVE_GOALS + 1 distinct goals with
    // priorities 1..=MAX_ACTIVE_GOALS+1, split across the two boards. The
    // single highest-priority-number goal (MAX_ACTIVE_GOALS + 1) must be the
    // one dropped; priorities 1..=MAX_ACTIVE_GOALS are all kept. Expressed
    // relative to MAX_ACTIVE_GOALS so it stays correct when the cap changes.
    let overflow_priority = (MAX_ACTIVE_GOALS + 1) as u32;
    let mut persisted_goals = Vec::new();
    let mut in_flight_goals = Vec::new();
    for priority in 1..=overflow_priority {
        let goal = goal_with(
            &format!("goal-{priority:03}-overflow"),
            priority,
            GoalProgress::NotStarted,
            "d",
        );
        if priority % 2 == 0 {
            persisted_goals.push(goal);
        } else {
            in_flight_goals.push(goal);
        }
    }
    let persisted = GoalBoard {
        active: persisted_goals,
        backlog: vec![],
    };
    let in_flight = GoalBoard {
        active: in_flight_goals,
        backlog: vec![],
    };
    let merged = merge_boards(persisted, in_flight);
    assert_eq!(merged.active.len(), MAX_ACTIVE_GOALS);
    let priorities: Vec<u32> = merged.active.iter().map(|g| g.priority).collect();
    assert!(
        !priorities.contains(&overflow_priority),
        "the highest-priority-number goal ({overflow_priority}) must be truncated, got {priorities:?}"
    );
    for p in 1..=(MAX_ACTIVE_GOALS as u32) {
        assert!(
            priorities.contains(&p),
            "priority {p} must be kept, got {priorities:?}"
        );
    }
}

/// (e) Active overflow tiebreak on equal priority → in-flight is preferred,
/// so a persisted goal must be the one dropped, never the in-flight one.
#[test]
fn merge_boards_overflow_tiebreak_prefers_in_flight() {
    // Fill the board to capacity with persisted goals, then add one in-flight
    // goal at the same priority so the union is MAX_ACTIVE_GOALS + 1 and the
    // tiebreak must drop a persisted goal, never the in-flight one.
    let persisted = GoalBoard {
        active: (0..MAX_ACTIVE_GOALS)
            .map(|i| {
                goal_with(
                    &format!("persisted-{i:02}-id"),
                    3,
                    GoalProgress::NotStarted,
                    "p",
                )
            })
            .collect(),
        backlog: vec![],
    };
    let in_flight = GoalBoard {
        active: vec![goal_with(
            "flight-only-id",
            3,
            GoalProgress::NotStarted,
            "f",
        )],
        backlog: vec![],
    };
    let merged = merge_boards(persisted, in_flight);
    assert_eq!(merged.active.len(), MAX_ACTIVE_GOALS);
    let ids: Vec<String> = merged.active.iter().map(|g| g.id.clone()).collect();
    assert!(
        ids.iter().any(|id| id == "flight-only-id"),
        "in-flight goal must survive on tiebreak, got {ids:?}"
    );
}

/// (f) Self-merge is identity at the field-set level (idempotent).
#[test]
fn merge_boards_self_merge_is_identity() {
    let board = GoalBoard {
        active: vec![
            goal_with(
                "self-aaaaa",
                1,
                GoalProgress::InProgress { percent: 10 },
                "a",
            ),
            goal_with("self-bbbbb", 2, GoalProgress::NotStarted, "b"),
        ],
        backlog: vec![backlog_with("self-bk", "x", "s", 0.5)],
    };
    let merged = merge_boards(board.clone(), board.clone());
    assert_eq!(merged.active.len(), board.active.len());
    assert_eq!(merged.backlog.len(), board.backlog.len());
    for g in &board.active {
        assert!(
            merged.active.iter().any(|m| m == g),
            "active goal {} must be preserved",
            g.id
        );
    }
    for b in &board.backlog {
        assert!(
            merged.backlog.iter().any(|m| m == b),
            "backlog item {} must be preserved",
            b.id
        );
    }
}

/// (g) Empty persisted → returns in-flight content unchanged.
#[test]
fn merge_boards_empty_persisted_returns_in_flight_unchanged() {
    let in_flight = GoalBoard {
        active: vec![goal_with("only-flight", 1, GoalProgress::NotStarted, "f")],
        backlog: vec![backlog_with("bk-flight", "x", "s", 0.1)],
    };
    let merged = merge_boards(GoalBoard::new(), in_flight.clone());
    assert_eq!(merged.active.len(), 1);
    assert_eq!(merged.active[0].id, "only-flight");
    assert_eq!(merged.backlog.len(), 1);
    assert_eq!(merged.backlog[0].id, "bk-flight");
}

/// (h) Empty in-flight → returns persisted content unchanged.
#[test]
fn merge_boards_empty_in_flight_returns_persisted_unchanged() {
    let persisted = GoalBoard {
        active: vec![goal_with(
            "only-persisted",
            1,
            GoalProgress::NotStarted,
            "p",
        )],
        backlog: vec![backlog_with("bk-persisted", "x", "s", 0.1)],
    };
    let merged = merge_boards(persisted.clone(), GoalBoard::new());
    assert_eq!(merged.active.len(), 1);
    assert_eq!(merged.active[0].id, "only-persisted");
    assert_eq!(merged.backlog.len(), 1);
    assert_eq!(merged.backlog[0].id, "bk-persisted");
}

/// (i) Cross-set collision (RR-5): id appears in persisted.active and
/// in_flight.backlog → in-flight classification wins, so it ends up in
/// backlog only.
#[test]
fn merge_boards_cross_set_collision_uses_in_flight_classification() {
    let persisted = GoalBoard {
        active: vec![goal_with(
            "shared-cross-id",
            1,
            GoalProgress::InProgress { percent: 50 },
            "from persisted active",
        )],
        backlog: vec![],
    };
    let in_flight = GoalBoard {
        active: vec![],
        backlog: vec![backlog_with(
            "shared-cross-id",
            "from in-flight backlog",
            "f",
            0.7,
        )],
    };
    let merged = merge_boards(persisted, in_flight);
    assert!(
        merged.active.iter().all(|g| g.id != "shared-cross-id"),
        "shared id must NOT appear in active after cross-set collision (got {:?})",
        merged
            .active
            .iter()
            .map(|g| g.id.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        merged.backlog.iter().any(|b| b.id == "shared-cross-id"),
        "shared id must appear in backlog (in-flight classification wins)"
    );
}

/// Determinism: repeated merges of the same inputs produce equal outputs.
#[test]
fn merge_boards_is_deterministic_across_runs() {
    let persisted = GoalBoard {
        active: vec![
            goal_with("zzz-goal-id", 2, GoalProgress::NotStarted, "z"),
            goal_with("aaa-goal-id", 2, GoalProgress::NotStarted, "a"),
        ],
        backlog: vec![],
    };
    let in_flight = GoalBoard {
        active: vec![goal_with("mmm-goal-id", 2, GoalProgress::NotStarted, "m")],
        backlog: vec![],
    };
    let merged_1 = merge_boards(persisted.clone(), in_flight.clone());
    let merged_2 = merge_boards(persisted.clone(), in_flight.clone());
    let merged_3 = merge_boards(persisted, in_flight);
    assert_eq!(merged_1, merged_2);
    assert_eq!(merged_2, merged_3);
}

// ── read_latest_snapshot: extracted helper ──────────────────────────────

/// `read_latest_snapshot` returns `None` when the memory has no snapshot.
#[test]
fn read_latest_snapshot_returns_none_when_empty() {
    let (memory, _facts) = stateful_memory();
    let result = read_latest_snapshot(&memory);
    assert!(
        result.is_none(),
        "expected None for empty store, got {result:?}"
    );
}

/// With multiple snapshot facts, `read_latest_snapshot` picks the one with
/// the largest `node_id` (most recent uuid-v7 / monotonic id).
#[test]
#[serial_test::serial(cognitive_memory)]
fn read_latest_snapshot_picks_max_node_id_when_multiple_present() {
    let (memory, _facts) = stateful_memory();
    let root = tmp_state_root("read-latest-multi");

    let first = GoalBoard {
        active: vec![goal_with(
            "first-saved-goal",
            1,
            GoalProgress::NotStarted,
            "first",
        )],
        backlog: vec![],
    };
    let second = GoalBoard {
        active: vec![
            goal_with("first-saved-goal", 1, GoalProgress::NotStarted, "first"),
            goal_with("second-saved-goal", 2, GoalProgress::NotStarted, "second"),
        ],
        backlog: vec![],
    };
    // Pre-fix save_goal_board appends two facts; post-fix it appends two
    // (merge of empty + first, then merge of first + second). Either way
    // there are ≥2 facts and the latest one must be returned.
    with_state_root(&root, || {
        super::save_goal_board(&first, &memory).unwrap();
        super::save_goal_board(&second, &memory).unwrap();
    });

    let latest =
        read_latest_snapshot(&memory).expect("must return Some when at least one snapshot exists");
    let ids: Vec<&str> = latest.active.iter().map(|g| g.id.as_str()).collect();
    assert!(
        ids.contains(&"second-saved-goal"),
        "latest snapshot must contain the most recently saved goal, got {ids:?}"
    );
}

/// `read_latest_snapshot` returns `None` (not Err / panic) when the memory
/// errors on search_facts. The caller (save_goal_board) uses this to fall
/// back to writing the in-flight board unchanged.
#[test]
fn read_latest_snapshot_returns_none_on_memory_search_error() {
    let memory = memory_search_fails();
    let result = read_latest_snapshot(&memory);
    assert!(
        result.is_none(),
        "search_facts error must surface as None, got {result:?}"
    );
}

// ── save_goal_board: merge-on-write integration tests ───────────────────

/// I1 — Sequential two writers with disjoint goal ids. After both saves,
/// `load_goal_board` returns BOTH goals. This is the canonical #1915
/// regression: pre-fix, the second save clobbered the first.
#[test]
#[serial_test::serial(cognitive_memory)]
fn save_goal_board_sequential_two_disjoint_writers_preserves_both() {
    let (memory, _facts) = stateful_memory();
    let root = tmp_state_root("save-seq-merge");

    let writer_a_board = GoalBoard {
        active: vec![goal_with(
            "alpha-writer-aaaa",
            1,
            GoalProgress::NotStarted,
            "Alpha goal",
        )],
        backlog: vec![],
    };
    let writer_b_board = GoalBoard {
        active: vec![goal_with(
            "beta-writer-bbbb",
            2,
            GoalProgress::NotStarted,
            "Beta goal",
        )],
        backlog: vec![],
    };

    with_state_root(&root, || {
        super::save_goal_board(&writer_a_board, &memory).unwrap();
        super::save_goal_board(&writer_b_board, &memory).unwrap();
        let loaded = super::load_goal_board(&memory).unwrap();
        let ids: Vec<&str> = loaded.active.iter().map(|g| g.id.as_str()).collect();
        assert!(
            ids.contains(&"alpha-writer-aaaa"),
            "writer A's goal must survive writer B's save (issue #1915), got {ids:?}"
        );
        assert!(
            ids.contains(&"beta-writer-bbbb"),
            "writer B's goal must be present, got {ids:?}"
        );
        assert_eq!(loaded.active.len(), 2);
    });
}

/// I2 — Sequential writes with same goal id → second write's fields win
/// (in-flight precedence on collision, applied at save time).
#[test]
#[serial_test::serial(cognitive_memory)]
fn save_goal_board_collision_persists_in_flight_fields() {
    let (memory, _facts) = stateful_memory();
    let root = tmp_state_root("save-collision");

    let first = GoalBoard {
        active: vec![goal_with(
            "collision-goal-idid",
            5,
            GoalProgress::NotStarted,
            "First desc",
        )],
        backlog: vec![],
    };
    let mut updated_goal = goal_with(
        "collision-goal-idid",
        1,
        GoalProgress::InProgress { percent: 80 },
        "Updated desc",
    );
    updated_goal.assigned_to = Some("engineer-9".to_string());
    let second = GoalBoard {
        active: vec![updated_goal],
        backlog: vec![],
    };

    with_state_root(&root, || {
        super::save_goal_board(&first, &memory).unwrap();
        super::save_goal_board(&second, &memory).unwrap();
        let loaded = super::load_goal_board(&memory).unwrap();
        assert_eq!(loaded.active.len(), 1);
        let g = &loaded.active[0];
        assert_eq!(g.description, "Updated desc");
        assert_eq!(g.priority, 1);
        assert!(
            matches!(g.status, GoalProgress::InProgress { percent: 80 }),
            "expected InProgress(80%), got {:?}",
            g.status
        );
        assert_eq!(g.assigned_to.as_deref(), Some("engineer-9"));
    });
}

/// I7 — Read-failure fallback: when `search_facts` errors, save_goal_board
/// must NOT panic and must NOT propagate the read error — it persists the
/// in-flight board unchanged (best-effort fail-open on read, per SR-6.1).
#[test]
#[serial_test::serial(cognitive_memory)]
fn save_goal_board_read_failure_falls_back_to_persisting_in_flight() {
    let recording = RpcRecording::shared();
    let memory = memory_search_fails_store_works(recording.clone());
    let root = tmp_state_root("save-readfail");

    let board = GoalBoard {
        active: vec![goal_with(
            "readfail-goal-idid",
            1,
            GoalProgress::NotStarted,
            "Fallback goal",
        )],
        backlog: vec![],
    };

    with_state_root(&root, || {
        super::save_goal_board(&board, &memory)
            .expect("read-failure must not propagate from save_goal_board");
    });

    let calls = recording.calls();
    assert_eq!(
        calls.len(),
        1,
        "in-flight board must still be persisted on read failure"
    );
    assert_eq!(calls[0].concept, "goal-board:snapshot");
    let persisted: GoalBoard = serde_json::from_str(&calls[0].content).unwrap();
    assert_eq!(persisted.active.len(), 1);
    assert_eq!(persisted.active[0].id, "readfail-goal-idid");
}

/// I4 — Capacity bound holds across many merge-on-write saves. Saving
/// MAX_ACTIVE_GOALS + 2 disjoint single-goal boards must result in a merged
/// board of exactly MAX_ACTIVE_GOALS goals (the ones with the lowest priority
/// numbers). Expressed relative to the cap so it stays correct if it changes.
#[test]
#[serial_test::serial(cognitive_memory)]
fn save_goal_board_capacity_bound_holds_after_multiple_merges() {
    let (memory, _facts) = stateful_memory();
    let root = tmp_state_root("save-capacity");

    with_state_root(&root, || {
        let overflow = MAX_ACTIVE_GOALS as u32 + 2;
        for i in 0u32..overflow {
            let board = GoalBoard {
                active: vec![goal_with(
                    &format!("capgoal-{i:04}-aaaa"),
                    i + 1,
                    GoalProgress::NotStarted,
                    "x",
                )],
                backlog: vec![],
            };
            super::save_goal_board(&board, &memory).unwrap();
        }
        let loaded = super::load_goal_board(&memory).unwrap();
        assert_eq!(
            loaded.active.len(),
            MAX_ACTIVE_GOALS,
            "merged board must be capped at MAX_ACTIVE_GOALS, got {} goals: {:?}",
            loaded.active.len(),
            loaded
                .active
                .iter()
                .map(|g| (g.id.as_str(), g.priority))
                .collect::<Vec<_>>()
        );
        let priorities: Vec<u32> = loaded.active.iter().map(|g| g.priority).collect();
        for p in 1..=(MAX_ACTIVE_GOALS as u32) {
            assert!(
                priorities.contains(&p),
                "priority {p} must be kept, got {priorities:?}"
            );
        }
        for p in (MAX_ACTIVE_GOALS as u32 + 1)..=overflow {
            assert!(
                !priorities.contains(&p),
                "priority {p} must be truncated, got {priorities:?}"
            );
        }
    });
}

// ── concurrent save_goal_board: regression test for #1915 ───────────────

/// Issue #1915 concurrency regression: two threads simultaneously
/// `save_goal_board` with disjoint single-goal boards against a shared
/// stateful memory. After both joins, `load_goal_board` MUST return a
/// board containing both goals. Repeated for 50 iterations with a Barrier
/// to maximise contention on the read-modify-write window.
#[test]
#[serial_test::serial(cognitive_memory)]
fn save_goal_board_concurrent_two_writers_preserves_both_goals() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    // Hold ENV_MUTEX for the entire test duration so concurrent tests
    // using `with_state_root` cannot unset SIMARD_STATE_ROOT mid-test.
    // Set SIMARD_STATE_ROOT explicitly (do NOT use HermeticState here —
    // HermeticState does not coordinate with ENV_MUTEX). Spawned threads
    // then inherit this env value when they read simard_state_root().
    let env_guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let root = tmp_state_root("save-concurrent");
    // SAFETY: serialised by ENV_MUTEX (and #[serial(cognitive_memory)]).
    unsafe {
        std::env::set_var("SIMARD_STATE_ROOT", &root);
    }
    // RAII restore on test exit (panic-safe).
    struct EnvRestore;
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var("SIMARD_STATE_ROOT");
            }
        }
    }
    let _restore = EnvRestore;

    for iter in 0..50u32 {
        let (memory, _facts) = stateful_memory();
        let memory: Arc<crate::memory_client::CognitiveMemoryClient> = Arc::new(memory);
        let barrier = Arc::new(Barrier::new(2));

        let alpha_id = format!("alpha-concur-{iter:04}");
        let beta_id = format!("beta-concur-{iter:04}");

        let memory_a = memory.clone();
        let barrier_a = barrier.clone();
        let alpha_id_thread = alpha_id.clone();
        let h_a = thread::spawn(move || {
            barrier_a.wait();
            let board = GoalBoard {
                active: vec![goal_with(
                    &alpha_id_thread,
                    1,
                    GoalProgress::NotStarted,
                    "Alpha",
                )],
                backlog: vec![],
            };
            super::save_goal_board(&board, memory_a.as_ref()).unwrap();
        });

        let memory_b = memory.clone();
        let barrier_b = barrier.clone();
        let beta_id_thread = beta_id.clone();
        let h_b = thread::spawn(move || {
            barrier_b.wait();
            let board = GoalBoard {
                active: vec![goal_with(
                    &beta_id_thread,
                    2,
                    GoalProgress::NotStarted,
                    "Beta",
                )],
                backlog: vec![],
            };
            super::save_goal_board(&board, memory_b.as_ref()).unwrap();
        });

        h_a.join().expect("alpha thread must not panic");
        h_b.join().expect("beta thread must not panic");

        // ENV_MUTEX is held for the whole test → calling with_state_root
        // here would deadlock. SIMARD_STATE_ROOT is already pinned, so
        // load_goal_board sees the correct value directly.
        let loaded = super::load_goal_board(memory.as_ref()).unwrap();
        let ids: Vec<String> = loaded.active.iter().map(|g| g.id.clone()).collect();
        assert!(
            ids.iter().any(|id| id == &alpha_id),
            "iter {iter}: alpha goal {alpha_id} disappeared — issue #1915 regression. ids={ids:?}"
        );
        assert!(
            ids.iter().any(|id| id == &beta_id),
            "iter {iter}: beta goal {beta_id} disappeared — issue #1915 regression. ids={ids:?}"
        );
    }
    drop(env_guard);
}

/// Companion concurrency test: backlog-only writes from two threads must
/// also preserve both items (backlog has the same drift risk as active
/// per requirement #2).
#[test]
#[serial_test::serial(cognitive_memory)]
fn save_goal_board_concurrent_backlog_writers_preserve_both_items() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    // Hold ENV_MUTEX for the entire test duration so concurrent tests
    // using `with_state_root` cannot unset SIMARD_STATE_ROOT mid-test
    // (see sibling test for full rationale).
    let env_guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let root = tmp_state_root("save-concurrent-backlog");
    // SAFETY: serialised by ENV_MUTEX (and #[serial(cognitive_memory)]).
    unsafe {
        std::env::set_var("SIMARD_STATE_ROOT", &root);
    }
    struct EnvRestore;
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var("SIMARD_STATE_ROOT");
            }
        }
    }
    let _restore = EnvRestore;

    for iter in 0..25u32 {
        let (memory, _facts) = stateful_memory();
        let memory: Arc<crate::memory_client::CognitiveMemoryClient> = Arc::new(memory);
        let barrier = Arc::new(Barrier::new(2));

        // Seed a guard-clean active goal so board_integrity_suspect passes
        // for both writers' boards (short ids would be rejected).
        let seed_goal = goal_with(
            &format!("seedgoal-{iter:04}-aa"),
            1,
            GoalProgress::NotStarted,
            "Seed",
        );

        let alpha_bk_id = format!("alpha-bk-{iter:04}");
        let beta_bk_id = format!("beta-bk-{iter:04}");

        let memory_a = memory.clone();
        let barrier_a = barrier.clone();
        let alpha_bk_id_t = alpha_bk_id.clone();
        let seed_a = seed_goal.clone();
        let h_a = thread::spawn(move || {
            barrier_a.wait();
            let board = GoalBoard {
                active: vec![seed_a],
                backlog: vec![backlog_with(&alpha_bk_id_t, "alpha-bk", "a", 0.3)],
            };
            super::save_goal_board(&board, memory_a.as_ref()).unwrap();
        });

        let memory_b = memory.clone();
        let barrier_b = barrier.clone();
        let beta_bk_id_t = beta_bk_id.clone();
        let seed_b = seed_goal.clone();
        let h_b = thread::spawn(move || {
            barrier_b.wait();
            let board = GoalBoard {
                active: vec![seed_b],
                backlog: vec![backlog_with(&beta_bk_id_t, "beta-bk", "b", 0.4)],
            };
            super::save_goal_board(&board, memory_b.as_ref()).unwrap();
        });

        h_a.join().unwrap();
        h_b.join().unwrap();

        // ENV_MUTEX is held → must not call with_state_root (would deadlock).
        let loaded = super::load_goal_board(memory.as_ref()).unwrap();
        let bk_ids: Vec<String> = loaded.backlog.iter().map(|b| b.id.clone()).collect();
        assert!(
            bk_ids.iter().any(|id| id == &alpha_bk_id),
            "iter {iter}: alpha backlog item {alpha_bk_id} disappeared, got {bk_ids:?}"
        );
        assert!(
            bk_ids.iter().any(|id| id == &beta_bk_id),
            "iter {iter}: beta backlog item {beta_bk_id} disappeared, got {bk_ids:?}"
        );
    }
    drop(env_guard);
}

#[test]
fn archive_completed_also_archives_in_progress_100() {
    let mut board = GoalBoard::new();
    add_active_goal(&mut board, make_goal("done-goal", 1)).unwrap();
    add_active_goal(&mut board, make_goal("wip-goal", 2)).unwrap();
    // Set one to InProgress { percent: 100 } — should also be archived
    update_goal_progress(
        &mut board,
        "done-goal",
        GoalProgress::InProgress { percent: 100 },
    )
    .unwrap();
    let archived = archive_completed(&mut board);
    assert_eq!(archived.len(), 1, "InProgress(100) should be archived");
    assert_eq!(archived[0].id, "done-goal");
    assert_eq!(board.active.len(), 1, "only wip-goal should remain");
}

// ── BoardWriteLock: cross-process write-lock primitive (issue #2511) ─────
//
// `save_goal_board` / `save_goal_board_with_removals` hold an in-process
// `SAVE_GOAL_BOARD_MUTEX` plus this advisory `flock` so the OODA daemon and a
// separate `simard goal` CLI process cannot interleave their read-merge-write
// windows. We test the lock primitive directly: `flock(2)` treats two
// independent open descriptions as mutually exclusive *even within one
// process*, so two threads each calling `acquire()` (each opening its own FD)
// faithfully model the cross-process daemon/CLI race the integrated lock
// guards against.

#[cfg(unix)]
#[test]
#[serial_test::serial(cognitive_memory)]
fn board_lock_path_resolves_under_state_root_state_dir() {
    let _env = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let root = tmp_state_root("board-lock-path");
    // SAFETY: serialised by ENV_MUTEX.
    unsafe { std::env::set_var("SIMARD_STATE_ROOT", &root) };
    let path = super::operations::board_lock_path();
    // SAFETY: serialised by ENV_MUTEX.
    unsafe { std::env::remove_var("SIMARD_STATE_ROOT") };
    assert_eq!(path, root.join("state").join("goal-board.lock"));
}

#[cfg(unix)]
#[test]
#[serial_test::serial(cognitive_memory)]
fn board_write_lock_serializes_independent_acquirers() {
    use std::sync::mpsc;
    use std::time::Duration;

    // Pin a hermetic state root for the whole test so `board_lock_path()`
    // resolves under a tempdir, not the operator's ~/.simard. Hold ENV_MUTEX
    // for the duration (mirrors the #1915 concurrency test).
    let _env = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let root = tmp_state_root("board-write-lock");
    // SAFETY: serialised by ENV_MUTEX.
    unsafe { std::env::set_var("SIMARD_STATE_ROOT", &root) };
    struct EnvRestore;
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            // SAFETY: serialised by ENV_MUTEX.
            unsafe { std::env::remove_var("SIMARD_STATE_ROOT") };
        }
    }
    let _restore = EnvRestore;

    // Acquire the first lock in this thread and hold it.
    let first = super::operations::BoardWriteLock::acquire()
        .expect("first lock acquisition should succeed");

    // A second, independent acquisition (its own FD, simulating a separate
    // process) must BLOCK until `first` is dropped.
    let (tx, rx) = mpsc::channel::<()>();
    let handle = std::thread::spawn(move || {
        let _second = super::operations::BoardWriteLock::acquire()
            .expect("second lock acquisition should eventually succeed");
        // Signal acquisition, then hold the lock until the receiver has
        // observed the signal (the guard drops at end of scope).
        tx.send(()).ok();
    });

    // While we still hold `first`, the second acquirer must not make progress.
    assert!(
        rx.recv_timeout(Duration::from_millis(250)).is_err(),
        "second acquirer must block while the first lock is held (#2511)"
    );

    // Release the first lock; the second acquirer can now proceed.
    drop(first);
    assert!(
        rx.recv_timeout(Duration::from_secs(5)).is_ok(),
        "second acquirer must obtain the lock once the first is released"
    );

    handle.join().expect("lock thread should join cleanly");
}

// ===========================================================================
// Standing/perpetual seed declaration + warm-board self-heal (issue #4927)
//
// TEST-FIRST for the un-shipped `standing` seed attribute and the
// `reconcile_standing_markers` warm-board self-heal. The live standing goal
// `articulate-repo-hygiene-backlog` was re-parked every OODA cycle and fed the
// `UNCLEAR-CRITERIA` issue storm (#4927/#4930/#4934) purely because it was
// never tagged perpetual — the no-progress breaker's `!is_perpetual()`
// exemption never fired for it. The fix is entirely in the seed-declaration +
// reconcile surface: a `standing = true` seed must produce a goal that reads as
// `is_perpetual()`, and a persisted (already-live) goal matching a standing
// seed must be self-healed idempotently, by exact id / normalized title-slug
// only — never by fuzzy prose (which would wrongly exempt a genuinely stuck
// goal from the safety breaker).
// ===========================================================================

const HYGIENE_TITLE: &str = "Articulate repo-hygiene backlog";
const HYGIENE_DESC: &str = "Turn observations into prioritized, target-scoped repo-hygiene goals on this identity's own board.";

fn standing_seed(title: &str, desc: &str) -> crate::identity::SeedGoal {
    crate::identity::SeedGoal::new(2, title, desc, None).standing()
}

fn ordinary_seed(title: &str, desc: &str) -> crate::identity::SeedGoal {
    crate::identity::SeedGoal::new(2, title, desc, None)
}

#[test]
fn seed_board_from_seed_goals_marks_a_standing_seed_as_perpetual() {
    // Cold start: a `standing = true` seed must produce an ActiveGoal that reads
    // as standing/perpetual (the single `is_perpetual()` predicate the breaker
    // exemption keys on), so #4927 never recurs on a fresh/re-seeded board.
    let mut board = GoalBoard::new();
    let added =
        seed_board_from_seed_goals(&mut board, &[standing_seed(HYGIENE_TITLE, HYGIENE_DESC)]);
    assert_eq!(added, 1);
    assert_eq!(board.active.len(), 1);
    assert!(
        board.active[0].is_perpetual(),
        "a standing=true seed must seed a perpetual goal (issue #4927)"
    );
}

#[test]
fn seed_board_from_seed_goals_leaves_an_ordinary_seed_non_perpetual() {
    // Regression guard: an ordinary (standing omitted) seed must remain a
    // convergence-required goal — the fix must NOT broadly reclassify goals.
    let mut board = GoalBoard::new();
    let added = seed_board_from_seed_goals(
        &mut board,
        &[ordinary_seed("Fix broken features", "audit specs")],
    );
    assert_eq!(added, 1);
    assert!(
        !board.active[0].is_perpetual(),
        "an ordinary seed must stay non-perpetual (no broad reclassification)"
    );
}

#[test]
fn reconcile_standing_markers_self_heals_a_persisted_goal_by_id() {
    // Warm board: the live `articulate-repo-hygiene-backlog` already sits on the
    // cognitive-memory board with an UNMARKED description (the #4927 defect). A
    // seed-only tag can't reach it (the empty-board guard no-ops), so a load-time
    // reconcile must stamp the standing marker onto the persisted goal whose id
    // matches the standing seed's slug — turning it perpetual in place.
    let id = crate::goals::goal_slug(HYGIENE_TITLE);
    let mut goal = ActiveGoal::new(id.clone(), HYGIENE_DESC, 2);
    goal.status = GoalProgress::NotStarted;
    assert!(
        !goal.is_perpetual(),
        "precondition: the live goal starts unmarked — the exact #4927 defect"
    );

    let mut board = GoalBoard::new();
    board.active.push(goal);

    let healed =
        reconcile_standing_markers(&mut board, &[standing_seed(HYGIENE_TITLE, HYGIENE_DESC)]);
    assert_eq!(
        healed, 1,
        "the matching persisted goal must be healed exactly once"
    );
    assert!(
        board.active[0].is_perpetual(),
        "reconcile must make the persisted hygiene goal read as perpetual (issue #4927)"
    );
}

#[test]
fn reconcile_standing_markers_matches_by_normalized_title_slug() {
    // Matching is by the NORMALIZED slug, not a byte-exact title, so a persisted
    // goal whose id came from a differently-cased/spaced title still self-heals.
    let id = "articulate-repo-hygiene-backlog".to_string();
    assert_eq!(
        crate::goals::goal_slug("Articulate  Repo-Hygiene   Backlog"),
        id,
        "slug normalization must collapse case/whitespace"
    );
    let mut goal = ActiveGoal::new(id, HYGIENE_DESC, 2);
    goal.status = GoalProgress::NotStarted;
    let mut board = GoalBoard::new();
    board.active.push(goal);

    let healed = reconcile_standing_markers(
        &mut board,
        &[standing_seed(
            "Articulate  Repo-Hygiene   Backlog",
            HYGIENE_DESC,
        )],
    );
    assert_eq!(healed, 1);
    assert!(board.active[0].is_perpetual());
}

#[test]
fn reconcile_standing_markers_is_idempotent() {
    // A second reconcile pass must be a no-op (returns 0) and must not
    // double-prepend the marker — `mark_standing` is idempotent by design.
    let id = crate::goals::goal_slug(HYGIENE_TITLE);
    let mut goal = ActiveGoal::new(id, HYGIENE_DESC, 2);
    goal.status = GoalProgress::NotStarted;
    let mut board = GoalBoard::new();
    board.active.push(goal);
    let seeds = [standing_seed(HYGIENE_TITLE, HYGIENE_DESC)];

    assert_eq!(reconcile_standing_markers(&mut board, &seeds), 1);
    let after_first = board.active[0].description.clone();
    assert_eq!(
        reconcile_standing_markers(&mut board, &seeds),
        0,
        "a second reconcile must heal nothing"
    );
    assert_eq!(
        board.active[0].description, after_first,
        "reconcile must not double-stamp the standing marker"
    );
}

#[test]
fn reconcile_standing_markers_ignores_unmatched_goals() {
    // Exact-match-only safety property: a goal whose id does NOT match any
    // standing seed must never be exempted from the no-progress breaker.
    let mut goal = ActiveGoal::new("some-other-goal", "unrelated work", 3);
    goal.status = GoalProgress::NotStarted;
    let mut board = GoalBoard::new();
    board.active.push(goal);

    let healed =
        reconcile_standing_markers(&mut board, &[standing_seed(HYGIENE_TITLE, HYGIENE_DESC)]);
    assert_eq!(healed, 0, "no non-matching goal may be stamped standing");
    assert!(!board.active[0].is_perpetual());
}

#[test]
fn reconcile_standing_markers_ignores_non_standing_seeds() {
    // A seed with standing=false must NEVER stamp a matching persisted goal —
    // otherwise every seed would silently become breaker-exempt.
    let id = crate::goals::goal_slug(HYGIENE_TITLE);
    let mut goal = ActiveGoal::new(id, HYGIENE_DESC, 2);
    goal.status = GoalProgress::NotStarted;
    let mut board = GoalBoard::new();
    board.active.push(goal);

    let healed =
        reconcile_standing_markers(&mut board, &[ordinary_seed(HYGIENE_TITLE, HYGIENE_DESC)]);
    assert_eq!(
        healed, 0,
        "an ordinary (standing=false) seed must heal nothing"
    );
    assert!(!board.active[0].is_perpetual());
}

#[test]
fn reconcile_standing_markers_skips_already_perpetual_goals() {
    // A goal already reading as perpetual must not be re-counted or re-stamped.
    let id = crate::goals::goal_slug(HYGIENE_TITLE);
    let goal = ActiveGoal::new(id, HYGIENE_DESC, 2).mark_standing();
    assert!(goal.is_perpetual());
    let before = goal.description.clone();
    let mut board = GoalBoard::new();
    board.active.push(goal);

    let healed =
        reconcile_standing_markers(&mut board, &[standing_seed(HYGIENE_TITLE, HYGIENE_DESC)]);
    assert_eq!(healed, 0, "an already-perpetual goal is not healed again");
    assert_eq!(board.active[0].description, before);
}
