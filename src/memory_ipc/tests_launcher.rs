//! Failing TDD tests (issue #1590, Step 7) for the cognitive-memory bridge
//! launcher helpers required by spec section A2 / Recommendation C.
//!
//! Public API under test (not yet implemented):
//!
//! ```ignore
//! pub struct WriterBridge { /* opaque */ }
//! pub struct ReaderBridge { /* opaque */ }
//!
//! impl WriterBridge { pub fn ops(&self) -> &dyn CognitiveMemoryOps; }
//! impl ReaderBridge { pub fn ops(&self) -> &dyn CognitiveMemoryOps; }
//!
//! pub fn launch_writer_bridge(state_root: &Path) -> SimardResult<WriterBridge>;
//! pub fn open_reader_bridge(state_root: &Path) -> SimardResult<ReaderBridge>;
//! ```
//!
//! Behavioural ladder for the writer (matches `launch_real_meeting_bridge`):
//!   1. Connect to the daemon's UDS at `default_socket_path()` if present.
//!   2. Otherwise reap any stale open-lock and `NativeCognitiveMemory::open`.
//!   3. Last-resort: `open_read_only` (typed as a writer here is a recoverable
//!      degradation — write attempts will surface errors at call time).
//!
//! Reader semantics: prefer the daemon socket; otherwise `open_read_only`
//! (which requires the underlying DB file to already exist).

use super::{launch_writer_bridge, open_reader_bridge};
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::goal_curation::{GoalBoard, load_goal_board, save_goal_board};

fn fresh_state_root(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "simard-launcher-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn launch_writer_bridge_succeeds_on_fresh_state_root_without_daemon() {
    // No daemon socket → must fall through to NativeCognitiveMemory::open.
    let root = fresh_state_root("writer-fresh");
    let writer = launch_writer_bridge(&root)
        .expect("launch_writer_bridge must succeed without a daemon when state root is writable");
    // ops() must hand back a usable trait object.
    let ops: &dyn CognitiveMemoryOps = writer.ops();
    let _ = ops
        .get_statistics()
        .expect("get_statistics must work on a fresh writer bridge");
}

#[test]
fn writer_bridge_supports_store_fact_round_trip() {
    let root = fresh_state_root("writer-roundtrip");
    let writer = launch_writer_bridge(&root).expect("writer bridge");

    writer
        .ops()
        .store_fact(
            "test-tdd-1590:roundtrip",
            "hello from TDD",
            1.0,
            &["tdd-1590".to_string()],
            "tdd-test",
        )
        .expect("store_fact through WriterBridge must succeed");

    let facts = writer
        .ops()
        .search_facts("test-tdd-1590:roundtrip", 5, 0.0)
        .expect("search_facts through WriterBridge must succeed");
    assert!(
        facts.iter().any(|f| f.content == "hello from TDD"),
        "round-tripped fact must be retrievable; got {} facts",
        facts.len()
    );
}

#[test]
fn open_reader_bridge_requires_existing_db() {
    // No DB has ever been opened → open_read_only would fail. The reader
    // helper is allowed to surface that as `Err`, but it must NOT panic
    // and must NOT silently succeed against a DB that was never created.
    let root = fresh_state_root("reader-missing");
    let result = open_reader_bridge(&root);
    assert!(
        result.is_err(),
        "open_reader_bridge against a never-initialised state root must return Err"
    );
}

#[test]
fn open_reader_bridge_succeeds_after_writer_initialises_db() {
    let root = fresh_state_root("reader-after-writer");
    {
        // Drop the writer to release the open-lock before the reader opens.
        let writer = launch_writer_bridge(&root).expect("writer bridge");
        writer
            .ops()
            .store_fact(
                "test-tdd-1590:reader-handoff",
                "seeded by writer",
                1.0,
                &["tdd-1590".to_string()],
                "tdd-test",
            )
            .expect("store_fact");
    }

    let reader = open_reader_bridge(&root)
        .expect("open_reader_bridge must succeed after writer has created the DB");
    let facts = reader
        .ops()
        .search_facts("test-tdd-1590:reader-handoff", 5, 0.0)
        .expect("reader search_facts");
    assert!(
        facts.iter().any(|f| f.content == "seeded by writer"),
        "reader bridge must surface facts written by the prior writer"
    );
}

#[test]
fn writer_bridge_is_compatible_with_save_and_load_goal_board() {
    // The whole point of these helpers is to let dashboard / meeting /
    // engineer call sites flow through `save_goal_board(&board, writer.ops())`
    // and `load_goal_board(reader.ops())` without any ceremony.
    let root = fresh_state_root("writer-goal-board");
    let writer = launch_writer_bridge(&root).expect("writer bridge");

    let mut board = GoalBoard::new();
    board.active.push(crate::goal_curation::ActiveGoal {
        id: "tdd-roundtrip-active-goal".to_string(),
        description: "Goal saved via WriterBridge then loaded again".to_string(),
        priority: 1,
        status: crate::goal_curation::GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });

    save_goal_board(&board, writer.ops()).expect("save_goal_board via WriterBridge must succeed");
    let loaded =
        load_goal_board(writer.ops()).expect("load_goal_board via WriterBridge must succeed");
    assert_eq!(loaded.active.len(), 1);
    assert_eq!(loaded.active[0].id, "tdd-roundtrip-active-goal");
}

#[test]
fn writer_bridge_does_not_create_legacy_goal_records_json_on_save() {
    // Acceptance criterion #6: every save must flow through cognitive memory.
    // No writer call site is allowed to create the legacy JSON file.
    let root = fresh_state_root("writer-no-disk-file");
    let writer = launch_writer_bridge(&root).expect("writer bridge");

    let mut board = GoalBoard::new();
    board.active.push(crate::goal_curation::ActiveGoal {
        id: "tdd-no-disk-file-goal".to_string(),
        description: "Saving a goal must not produce goal_records.json".to_string(),
        priority: 1,
        status: crate::goal_curation::GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
    });
    save_goal_board(&board, writer.ops()).expect("save_goal_board");

    let legacy = root.join("goal_records.json");
    assert!(
        !legacy.exists(),
        "save_goal_board through WriterBridge must NOT create {}",
        legacy.display()
    );
}
