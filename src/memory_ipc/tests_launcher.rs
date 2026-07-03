//! Failing TDD tests (issue #1590, Step 7) for the cognitive-memory adapter
//! launcher helpers required by spec section A2 / Recommendation C.
//!
//! Public API under test (not yet implemented):
//!
//! ```ignore
//! pub struct WriterAdapter { /* opaque */ }
//! pub struct ReaderAdapter { /* opaque */ }
//!
//! impl WriterAdapter { pub fn ops(&self) -> &dyn CognitiveMemoryOps; }
//! impl ReaderAdapter { pub fn ops(&self) -> &dyn CognitiveMemoryOps; }
//!
//! pub fn launch_writer_adapter(state_root: &Path) -> SimardResult<WriterAdapter>;
//! pub fn open_reader_adapter(state_root: &Path) -> SimardResult<ReaderAdapter>;
//! ```
//!
//! Behavioural ladder for the writer (matches `launch_real_meeting_adapter`):
//!   1. Connect to the daemon's UDS at `default_socket_path()` if present.
//!   2. Otherwise reap any stale open-lock and `LibraryCognitiveMemory::open`.
//!
//! Reader semantics (de-fork Phase 2b): prefer the daemon socket; otherwise a
//! direct `LibraryCognitiveMemory::open` (the library has no read-only mode, so
//! the reader creates the store if it does not yet exist).

use super::{
    clear_in_process_writer, launch_writer_adapter, open_reader_adapter, register_in_process_writer,
};
use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::goal_curation::{GoalBoard, load_goal_board, save_goal_board};
use std::sync::Arc;

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
#[serial_test::serial(cognitive_memory)]
fn launch_writer_adapter_succeeds_on_fresh_state_root_without_daemon() {
    // No daemon socket → must fall through to LibraryCognitiveMemory::open.
    let root = fresh_state_root("writer-fresh");
    let writer = launch_writer_adapter(&root)
        .expect("launch_writer_adapter must succeed without a daemon when state root is writable");
    // ops() must hand back a usable trait object.
    let ops: &dyn CognitiveMemoryOps = writer.ops();
    let _ = ops
        .get_statistics()
        .expect("get_statistics must work on a fresh writer adapter");
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn writer_adapter_supports_store_fact_round_trip() {
    let root = fresh_state_root("writer-roundtrip");
    let writer = launch_writer_adapter(&root).expect("writer adapter");

    writer
        .ops()
        .store_fact(
            "test-tdd-1590:roundtrip",
            "hello from TDD",
            1.0,
            &["tdd-1590".to_string()],
            "tdd-test",
        )
        .expect("store_fact through WriterAdapter must succeed");

    let facts = writer
        .ops()
        .search_facts("test-tdd-1590:roundtrip", 5, 0.0)
        .expect("search_facts through WriterAdapter must succeed");
    assert!(
        facts.iter().any(|f| f.content == "hello from TDD"),
        "round-tripped fact must be retrievable; got {} facts",
        facts.len()
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn open_reader_adapter_creates_empty_store_when_missing() {
    // De-fork Phase 2b (issue #2307): the library backend has no read-only
    // constructor, so the reader's tier-2 direct open CREATES the store when it
    // does not yet exist (rather than the native `open_read_only`, which failed
    // on a missing DB). The contract is now: `open_reader_adapter` succeeds and
    // returns an empty, queryable store.
    let root = fresh_state_root("reader-missing");
    let reader =
        open_reader_adapter(&root).expect("open_reader_adapter must create an empty store");
    let stats = reader
        .ops()
        .get_statistics()
        .expect("get_statistics on a freshly created store must succeed");
    assert_eq!(
        stats.semantic_count, 0,
        "a freshly created store must hold no facts"
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn open_reader_adapter_succeeds_after_writer_initialises_db() {
    let root = fresh_state_root("reader-after-writer");
    {
        // Drop the writer to release the open-lock before the reader opens.
        let writer = launch_writer_adapter(&root).expect("writer adapter");
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

    let reader = open_reader_adapter(&root)
        .expect("open_reader_adapter must succeed after writer has created the DB");
    let facts = reader
        .ops()
        .search_facts("test-tdd-1590:reader-handoff", 5, 0.0)
        .expect("reader search_facts");
    assert!(
        facts.iter().any(|f| f.content == "seeded by writer"),
        "reader adapter must surface facts written by the prior writer"
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn writer_adapter_is_compatible_with_save_and_load_goal_board() {
    // The whole point of these helpers is to let dashboard / meeting /
    // engineer call sites flow through `save_goal_board(&board, writer.ops())`
    // and `load_goal_board(reader.ops())` without any ceremony.
    // HermeticState pins SIMARD_STATE_ROOT to a TempDir so the
    // #[cfg(test)] hermetic guard in save_goal_board does not trip.
    let hermetic = crate::test_support::HermeticState::new();
    let root = hermetic.state_root().to_path_buf();
    let writer = launch_writer_adapter(&root).expect("writer adapter");

    let mut board = GoalBoard::new();
    board.active.push(crate::goal_curation::ActiveGoal {
        parent_goal_id: None,
        repo: None,
        id: "tdd-roundtrip-active-goal".to_string(),
        description: "Goal saved via WriterAdapter then loaded again".to_string(),
        priority: 1,
        status: crate::goal_curation::GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    });

    save_goal_board(&board, writer.ops()).expect("save_goal_board via WriterAdapter must succeed");
    let loaded =
        load_goal_board(writer.ops()).expect("load_goal_board via WriterAdapter must succeed");
    assert_eq!(loaded.active.len(), 1);
    assert_eq!(loaded.active[0].id, "tdd-roundtrip-active-goal");
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn writer_adapter_does_not_create_legacy_goal_records_json_on_save() {
    // Acceptance criterion #6: every save must flow through cognitive memory.
    // No writer call site is allowed to create the legacy JSON file.
    // HermeticState pins SIMARD_STATE_ROOT to a TempDir so the
    // #[cfg(test)] hermetic guard in save_goal_board does not trip.
    let hermetic = crate::test_support::HermeticState::new();
    let root = hermetic.state_root().to_path_buf();
    let writer = launch_writer_adapter(&root).expect("writer adapter");

    let mut board = GoalBoard::new();
    board.active.push(crate::goal_curation::ActiveGoal {
        parent_goal_id: None,
        repo: None,
        id: "tdd-no-disk-file-goal".to_string(),
        description: "Saving a goal must not produce goal_records.json".to_string(),
        priority: 1,
        status: crate::goal_curation::GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    });
    save_goal_board(&board, writer.ops()).expect("save_goal_board");

    let legacy = root.join("goal_records.json");
    assert!(
        !legacy.exists(),
        "save_goal_board through WriterAdapter must NOT create {}",
        legacy.display()
    );
}

// ---------------------------------------------------------------------------
// Issue #1590 follow-up — TDD tests for the dashboard hollow-success bug.
//
// The dashboard runs in the same process as the OODA daemon. Historically the
// launcher walked tiers 1 → 2 → 3 and the (now-removed) tier 3 was the bug:
//
//   1. IPC to ~/.simard/memory.sock — fails when the daemon's own writer
//      thread is already serving the request from the same process and
//      the connection self-deadlocks (or when state_root_matches_daemon
//      returns false for non-canonicalised paths).
//   2. LibraryCognitiveMemory::open — fails because the daemon owns the
//      writer lock.
//   3. (removed) open_read_only — used to succeed, returning a read-only
//      handle wrapped as a `WriterAdapter`. Subsequent writes silently no-op
//      at the IPC transport. This silent-degradation tier was deleted.
//
// The fix:
//   - Tier 0: in-process Arc shortcut, registered by the daemon at
//     startup. Same-process callers skip IPC entirely.
//   - Remove tier 3 (silent read-only fallback).
//   - Defensive `is_read_only()` invariant on `WriterAdapter`.
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial(cognitive_memory)]
fn register_in_process_writer_returns_registered_arc_via_launch_writer_adapter() {
    // Use an in-memory LibraryCognitiveMemory so we don't depend on disk
    // state. The state_root passed to launch_writer_adapter must match
    // the registered state_root for the shortcut to fire (path-aware
    // registration so unrelated tests with different state_roots are
    // unaffected).
    clear_in_process_writer();

    let inner: Arc<dyn CognitiveMemoryOps> = Arc::new(
        LibraryCognitiveMemory::in_memory()
            .expect("in-memory LibraryCognitiveMemory must construct for tests"),
    );

    let root = fresh_state_root("in-process-writer-shortcut");
    register_in_process_writer(root.clone(), Arc::clone(&inner));

    // Call launch_writer_adapter with the registered state_root — without
    // the in-process shortcut, tier 2 would create a fresh DB on disk at
    // this path. With the shortcut, the launcher returns the registered
    // Arc and never touches disk.
    let writer = launch_writer_adapter(&root)
        .expect("launch_writer_adapter must succeed via the registered in-process writer");

    // Write through the adapter.
    writer
        .ops()
        .store_fact(
            "tdd-1590:in-process-writer",
            "written via launch_writer_adapter after register",
            1.0,
            &["tdd-1590".to_string()],
            "tdd-test",
        )
        .expect("store_fact through in-process writer must succeed");

    // The fact must be visible on the registered Arc directly,
    // proving the adapter and the registered handle are the SAME backend.
    let facts = inner
        .search_facts("tdd-1590:in-process-writer", 5, 0.0)
        .expect("search_facts on the registered Arc must succeed");
    assert!(
        facts
            .iter()
            .any(|f| f.content == "written via launch_writer_adapter after register"),
        "the in-process shortcut must route writes to the registered Arc; got {} facts",
        facts.len()
    );

    // The registered shortcut must also avoid creating a DB on disk
    // at the (irrelevant) state_root passed to launch_writer_adapter.
    let db_path = root.join("cognitive");
    assert!(
        !db_path.exists(),
        "tier-0 shortcut must NOT create an on-disk DB at {}",
        db_path.display()
    );

    // Cleanup so other tests don't see the registration.
    clear_in_process_writer();
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn launch_writer_adapter_returns_err_when_state_root_is_unwritable_file() {
    // Force tiers 1 and 2 to fail by passing a path that is a regular
    // file rather than a directory. The launcher's tier 2
    // (LibraryCognitiveMemory::open) fails because the path is not a
    // usable directory — the user-visible result is `Err`. There is no
    // read-only fallback tier; the failure surfaces from tier 2.
    //
    // Either way, the contract this test pins is: the launcher must
    // never silently return a `WriterAdapter` whose underlying handle
    // cannot perform writes against the requested state_root.
    let parent = fresh_state_root("writer-unwritable-parent");
    let unwritable = parent.join("not-a-dir.txt");
    std::fs::write(&unwritable, b"this is a regular file, not a directory").expect("seed file");

    let result = launch_writer_adapter(&unwritable);
    assert!(
        result.is_err(),
        "launch_writer_adapter must return Err for an unusable state_root, \
         got Ok writer (regression: silent read-only fallback or hollow success)"
    );
}
