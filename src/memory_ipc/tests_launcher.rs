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

use super::{launch_writer_bridge, open_reader_bridge, register_in_process_writer};
use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
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

// ---------------------------------------------------------------------------
// Issue #1590 follow-up — TDD tests for the dashboard hollow-success bug.
//
// The dashboard runs in the same process as the OODA daemon. When it calls
// `launch_writer_bridge`, the launcher today walks tiers 1 → 2 → 3:
//
//   1. IPC to ~/.simard/memory.sock — fails when the daemon's own writer
//      thread is already serving the request from the same process and
//      the connection self-deadlocks (or when state_root_matches_daemon
//      returns false for non-canonicalised paths).
//   2. NativeCognitiveMemory::open — fails because the daemon owns the
//      writer flock.
//   3. open_read_only — succeeds, returns a read-only handle wrapped as
//      a `WriterBridge`. Subsequent writes silently no-op at the IPC
//      transport (or surface BridgeTransportError that the dashboard
//      handler converts into Json({"error": …}) but only after the
//      Ok(()) path has been threaded through `dashboard_save_goal_board`).
//
// The fix:
//   - Tier 0: in-process Arc shortcut, registered by the daemon at
//     startup. Same-process callers skip IPC entirely.
//   - Remove tier 3 (silent read-only fallback).
//   - Defensive `is_read_only()` invariant on `WriterBridge`.
// ---------------------------------------------------------------------------

#[test]
fn register_in_process_writer_returns_registered_arc_via_launch_writer_bridge() {
    // Use an in-memory NativeCognitiveMemory so we don't depend on disk
    // state. The state_root passed to launch_writer_bridge is irrelevant
    // when the in-process writer is registered: the launcher must short-
    // circuit and return the registered Arc.
    let inner: Arc<dyn CognitiveMemoryOps> = Arc::new(
        NativeCognitiveMemory::in_memory()
            .expect("in-memory NativeCognitiveMemory must construct for tests"),
    );

    register_in_process_writer(Arc::clone(&inner));

    // Call launch_writer_bridge with a state_root that has nothing on
    // disk — without the in-process shortcut, tier 2 would create a
    // fresh DB at this path. With the shortcut, the launcher returns
    // the registered Arc and never touches disk.
    let root = fresh_state_root("in-process-writer-shortcut");
    let writer = launch_writer_bridge(&root)
        .expect("launch_writer_bridge must succeed via the registered in-process writer");

    // Write through the bridge.
    writer
        .ops()
        .store_fact(
            "tdd-1590:in-process-writer",
            "written via launch_writer_bridge after register",
            1.0,
            &["tdd-1590".to_string()],
            "tdd-test",
        )
        .expect("store_fact through in-process writer must succeed");

    // The fact must be visible on the registered Arc directly,
    // proving the bridge and the registered handle are the SAME backend.
    let facts = inner
        .search_facts("tdd-1590:in-process-writer", 5, 0.0)
        .expect("search_facts on the registered Arc must succeed");
    assert!(
        facts
            .iter()
            .any(|f| f.content == "written via launch_writer_bridge after register"),
        "the in-process shortcut must route writes to the registered Arc; got {} facts",
        facts.len()
    );

    // The registered shortcut must also avoid creating a DB file on disk
    // at the (irrelevant) state_root passed to launch_writer_bridge.
    let db_path = root.join("cognitive_memory.ladybug");
    assert!(
        !db_path.exists(),
        "tier-0 shortcut must NOT create an on-disk DB at {}",
        db_path.display()
    );
}

#[test]
fn writer_bridge_construction_panics_when_inner_is_read_only() {
    // Defensive invariant: WriterBridge must refuse to wrap a read-only
    // handle. We construct a NativeCognitiveMemory via open() to create
    // the DB, drop it, then re-open read-only and assert that the
    // launcher (or whatever path constructs a WriterBridge from the
    // read-only handle) panics with a clear message.
    use crate::memory_ipc::WriterBridge;

    let root = fresh_state_root("writer-bridge-readonly-guard");
    {
        let _writer = launch_writer_bridge(&root).expect("seed the DB");
    }
    let ro = NativeCognitiveMemory::open_read_only(&root).expect("open read-only");
    assert!(
        ro.is_read_only(),
        "open_read_only must report is_read_only() == true"
    );

    // Construct a WriterBridge directly from the read-only handle. The
    // assertion in WriterBridge's constructor must fire — a writer
    // bridge wrapping a read-only handle is exactly the silent-
    // degradation hazard the fix eliminates.
    //
    // `NativeCognitiveMemory` is not `RefUnwindSafe` (its inner
    // `lbug::Database` wraps an `UnsafeCell`), so wrap the call in
    // `AssertUnwindSafe` — we are the sole owner of `ro` here, and the
    // panic we want to assert against happens before any state can be
    // observed.
    let ro_box: Box<dyn CognitiveMemoryOps> = Box::new(ro);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        WriterBridge::from_ops_for_test(ro_box)
    }));
    assert!(
        result.is_err(),
        "WriterBridge construction must panic when wrapping a read-only handle"
    );
}

#[test]
fn launch_writer_bridge_returns_err_when_state_root_is_unwritable_file() {
    // Force tiers 1 and 2 to fail by passing a path that is a regular
    // file rather than a directory. Today the launcher falls through
    // to tier 3 (open_read_only) which itself fails because the file
    // is not a LadybugDB — the user-visible result is `Err`. After the
    // fix tier 3 is removed entirely; the failure surfaces from tier 2
    // with a clearer message but still as `Err`.
    //
    // Either way, the contract this test pins is: the launcher must
    // never silently return a `WriterBridge` whose underlying handle
    // cannot perform writes against the requested state_root.
    let parent = fresh_state_root("writer-unwritable-parent");
    let unwritable = parent.join("not-a-dir.txt");
    std::fs::write(&unwritable, b"this is a regular file, not a directory").expect("seed file");

    let result = launch_writer_bridge(&unwritable);
    assert!(
        result.is_err(),
        "launch_writer_bridge must return Err for an unusable state_root, \
         got Ok writer (regression: silent read-only fallback or hollow success)"
    );
}

#[test]
fn native_cognitive_memory_open_read_only_reports_is_read_only_true() {
    // Trait-default contract: writers report false, the read-only
    // opener reports true. Pin both so any future regression that
    // forgets to override `is_read_only` for a read-only backend is
    // caught immediately.
    let root = fresh_state_root("is-read-only-trait");
    {
        let writer = launch_writer_bridge(&root).expect("seed DB");
        assert!(
            !writer.ops().is_read_only(),
            "writer bridge must report is_read_only() == false"
        );
    }
    let ro = NativeCognitiveMemory::open_read_only(&root).expect("open read-only");
    assert!(
        ro.is_read_only(),
        "NativeCognitiveMemory::open_read_only must report is_read_only() == true"
    );
}
