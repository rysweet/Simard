//! TDD (Step 7) — fail-closed goal-store persistence for bug #2896.
//!
//! Bug #2896: the creative-ideas thread routes accepted ideas to goals, the
//! write path reports success ("N → goal, 0 review error(s)"), yet ZERO goals
//! labelled `source:creative-ideas` are visible via `simard goal list`. This is
//! silent data loss. The confirmed mechanism on this branch is that
//! [`CognitiveMemoryGoalStore::list`] SWALLOWS reader/transport errors and
//! returns `Ok(Vec::new())` — so a broken-pipe read makes a persisted goal look
//! absent, and a caller that trusts the empty list drops the record.
//!
//! These tests pin the fail-closed contract mandated by #2896 (NO silent-failure
//! fallback anywhere):
//!   * A reader/transport error surfaced during `list()` MUST propagate as `Err`
//!     — never a phantom empty result. (RED before the fix.)
//!   * A writer/transport error during `put()` MUST propagate as `Err` — never a
//!     phantom `Ok`. (Guard: already fail-closed; must stay so.)
//!
//! Hermetic: a fault-injecting [`FaultyMemory`] is registered as the in-process
//! writer (tier 0) for a `TempDir` state root, so both `put` and `list` route to
//! it with no daemon, no socket, and no disk store. The registration mutates
//! process-global state, so every test is `#[serial_test::serial(cognitive_memory)]`
//! and clears the registration on entry and exit.

use std::sync::Arc;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::goals::{CognitiveMemoryGoalStore, GoalRecord, GoalStatus, GoalStore, GoalUpdate};
use crate::memory_cognitive::{
    CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};
use crate::memory_ipc::{clear_in_process_writer, register_in_process_writer};
use crate::session::{SessionId, SessionPhase};

/// A cognitive-memory backend that simulates a memory-ipc transport failure on
/// the read and/or write path, mirroring the real "bridge 'memory-ipc' transport
/// error: write-len: Broken pipe" the bug reports. Non-faulted operations return
/// benign values so the store can reach the faulted call.
struct FaultyMemory {
    /// Fault the read path (`search_facts`), simulating a broken-pipe read.
    fail_reads: bool,
    /// Fault the write path (`store_fact*`), simulating a broken-pipe write.
    fail_writes: bool,
}

impl FaultyMemory {
    fn transport_err(op: &str) -> SimardError {
        // Same shape the real IPC client surfaces so the assertion reflects the
        // production error, not a bespoke test-only variant.
        SimardError::RpcCallFailed {
            endpoint: "memory-ipc".to_string(),
            method: op.to_string(),
            reason: "write-len: Broken pipe (os error 32)".to_string(),
        }
    }
}

impl CognitiveMemoryOps for FaultyMemory {
    fn record_sensory(&self, _m: &str, _r: &str, _t: u64) -> SimardResult<String> {
        Ok("sensory".to_string())
    }
    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        Ok(0)
    }
    fn push_working(&self, _s: &str, _c: &str, _t: &str, _r: f64) -> SimardResult<String> {
        Ok("working".to_string())
    }
    fn get_working(&self, _t: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        Ok(vec![])
    }
    fn clear_working(&self, _t: &str) -> SimardResult<usize> {
        Ok(0)
    }
    fn store_episode(
        &self,
        _c: &str,
        _s: &str,
        _m: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        Ok("episode".to_string())
    }
    fn consolidate_episodes(&self, _b: u32) -> SimardResult<Option<String>> {
        Ok(None)
    }
    fn store_fact(
        &self,
        _c: &str,
        _co: &str,
        _cf: f64,
        _t: &[String],
        _s: &str,
    ) -> SimardResult<String> {
        if self.fail_writes {
            return Err(Self::transport_err("store_fact"));
        }
        Ok("fact".to_string())
    }
    fn store_fact_with_caller_key(
        &self,
        _caller_key: &str,
        _concept: &str,
        _content: &str,
        _confidence: f64,
        _tags: &[String],
        _source_id: &str,
    ) -> SimardResult<String> {
        if self.fail_writes {
            return Err(Self::transport_err("store_fact_with_caller_key"));
        }
        Ok("fact".to_string())
    }
    fn search_facts(&self, _q: &str, _l: u32, _m: f64) -> SimardResult<Vec<CognitiveFact>> {
        if self.fail_reads {
            return Err(Self::transport_err("search_facts"));
        }
        Ok(vec![])
    }
    fn store_procedure(&self, _n: &str, _s: &[String], _p: &[String]) -> SimardResult<String> {
        Ok("procedure".to_string())
    }
    fn recall_procedure(&self, _q: &str, _l: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        Ok(vec![])
    }
    fn store_prospective(&self, _d: &str, _tc: &str, _a: &str, _p: i64) -> SimardResult<String> {
        Ok("prospective".to_string())
    }
    fn check_triggers(&self, _c: &str) -> SimardResult<Vec<CognitiveProspective>> {
        Ok(vec![])
    }
    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        Ok(CognitiveStatistics::default())
    }
}

/// A `TempDir` state root (never under `$HOME/.simard`, so the hermetic guard is
/// satisfied) paired with a fault backend registered as the in-process writer.
struct FaultFixture {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    // Held so the registered `Weak` upgrades for the life of the test.
    _mem: Arc<dyn CognitiveMemoryOps>,
}

fn register_fault_writer(fail_reads: bool, fail_writes: bool) -> FaultFixture {
    clear_in_process_writer();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let mem: Arc<dyn CognitiveMemoryOps> = Arc::new(FaultyMemory {
        fail_reads,
        fail_writes,
    });
    register_in_process_writer(root.clone(), Arc::clone(&mem));
    FaultFixture {
        _dir: dir,
        root,
        _mem: mem,
    }
}

fn sample_goal() -> GoalRecord {
    GoalRecord::from_update(
        GoalUpdate::new(
            "Persist creative-idea goals durably",
            "goals routed from creative ideas must not silently vanish",
            GoalStatus::Proposed,
            3,
        )
        .expect("goal update should be valid"),
        "simard",
        SessionId::parse("session-018f1f7e-4c5d-7b2a-8f10-b5c0d4f7b123")
            .expect("session id should parse"),
        SessionPhase::Planning,
    )
    .expect("goal record should be valid")
}

/// RED (fails before the #2896 fix): a reader/transport error during `list()`
/// MUST surface as `Err`. Before the fix, `list_via_reader` catches the
/// `search_facts` error and returns `Ok(Vec::new())`, so the goal that was just
/// written looks like it was never there — the exact silent-loss signature of
/// #2896 (`goal list` returns 0 of an accepted idea's goals).
#[test]
#[serial_test::serial(cognitive_memory)]
fn list_surfaces_reader_transport_error_instead_of_phantom_empty() {
    let fx = register_fault_writer(/*fail_reads=*/ true, /*fail_writes=*/ false);
    let store = CognitiveMemoryGoalStore::new(fx.root.clone()).expect("construct goal store");

    let result = store.list();

    clear_in_process_writer();
    assert!(
        result.is_err(),
        "a broken-pipe read MUST propagate as Err, never a phantom empty list \
         (silent goal loss — bug #2896). Got: {result:?}",
    );
}

/// RED companion: the same fail-closed guarantee must hold for the higher-level
/// `active_top_goals` read used by the live goal board, so a transport fault can
/// never masquerade as "no active goals".
#[test]
#[serial_test::serial(cognitive_memory)]
fn active_top_goals_surfaces_reader_transport_error() {
    let fx = register_fault_writer(/*fail_reads=*/ true, /*fail_writes=*/ false);
    let store = CognitiveMemoryGoalStore::new(fx.root.clone()).expect("construct goal store");

    let result = store.active_top_goals(5);

    clear_in_process_writer();
    assert!(
        result.is_err(),
        "active_top_goals MUST propagate a reader transport error, not silently \
         report zero goals (bug #2896). Got: {result:?}",
    );
}

/// Guard (already fail-closed; must stay so): a writer/transport error during
/// `put()` MUST propagate as `Err`. The bug's "0 review error(s)" telemetry means
/// a phantom `Ok` here would let the router increment `routed_goal` for a write
/// that never landed. `put()` calls `store_fact_with_caller_key` first, so a
/// faulted write must abort the whole `put`.
#[test]
#[serial_test::serial(cognitive_memory)]
fn put_surfaces_writer_transport_error_instead_of_phantom_ok() {
    let fx = register_fault_writer(/*fail_reads=*/ false, /*fail_writes=*/ true);
    let store = CognitiveMemoryGoalStore::new(fx.root.clone()).expect("construct goal store");

    let result = store.put(sample_goal());

    clear_in_process_writer();
    assert!(
        result.is_err(),
        "a broken-pipe write MUST propagate as Err so the route is counted as a \
         failure, not a phantom success (bug #2896). Got: {result:?}",
    );
}
