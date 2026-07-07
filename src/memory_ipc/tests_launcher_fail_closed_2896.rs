//! TDD (Step 7) — the memory-ipc launcher must fail closed for bug #2896.
//!
//! Bug #2896 traces silent creative-ideas goal loss in part to the launcher's
//! silent degradation: when the daemon socket is PRESENT but the connection
//! cannot be established, `launch_writer_client` / `open_reader_client` today log
//! an `eprintln!` and fall through to a DIVERGENT direct (tier-2) open. A caller
//! then writes to a store that is not the one the live daemon reads, so the write
//! "succeeds" but is invisible — exactly the silent-failure fallback #2896
//! forbids.
//!
//! Contract pinned here (fail-closed; NO silent-failure fallback):
//!   * RED: socket path OCCUPIED by a non-socket file (connect impossible) →
//!     writer/reader launch MUST return `Err`, not a phantom tier-2 handle.
//!   * GUARD: genuinely absent socket (hermetic tests / standalone CLI) → tier-2
//!     is the legitimate path and MUST still succeed.
//!   * GUARD: an established memory-ipc connection whose backend op fails
//!     (broken-pipe / backend error) MUST surface as `Err` from the client op —
//!     it must never be swallowed into a phantom `Ok`.
//!
//! Hermetic: `TempDir` state roots (never under `$HOME/.simard`), no live daemon.
//! Global tier-0 / tier-2 caches are process-wide, so tests are
//! `#[serial_test::serial(cognitive_memory)]` and reset the caches.

use std::sync::Arc;
use std::time::Duration;

use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

use super::{
    clear_in_process_writer, clear_tier2_store_cache, launch_writer_client, open_reader_client,
    socket_path_for, spawn_server,
};
use crate::cognitive_memory::CognitiveMemoryOps;

/// Occupy the socket path with a plain file so `sock.exists()` is true but
/// `RemoteCognitiveMemory::connect` fails (a non-socket occupant is not a
/// reap-able stale daemon socket and is not the "no socket" case — it is a
/// present-but-unusable endpoint that must NOT be papered over with a divergent
/// tier-2 open).
fn occupy_socket_path_with_regular_file(root: &std::path::Path) {
    let sock = socket_path_for(root);
    if let Some(parent) = sock.parent() {
        std::fs::create_dir_all(parent).expect("create socket parent dir");
    }
    std::fs::write(&sock, b"not a socket").expect("write placeholder file at socket path");
}

/// RED (fails before the #2896 fix): a present-but-unconnectable socket must make
/// the WRITER launch fail closed rather than silently opening a divergent tier-2
/// store the live daemon never sees.
#[test]
#[serial_test::serial(cognitive_memory)]
fn launch_writer_client_fails_closed_when_socket_present_but_unconnectable() {
    clear_in_process_writer();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    occupy_socket_path_with_regular_file(root);

    let result = launch_writer_client(root);

    clear_tier2_store_cache();
    assert!(
        result.is_err(),
        "socket present but unconnectable MUST fail closed — never silently fall \
         through to a divergent tier-2 writer (bug #2896: silent-failure fallback). \
         Got Ok(_)",
    );
}

/// RED (fails before the #2896 fix): same fail-closed contract for the READER —
/// a divergent tier-2 reader is how a persisted goal becomes invisible.
#[test]
#[serial_test::serial(cognitive_memory)]
fn open_reader_client_fails_closed_when_socket_present_but_unconnectable() {
    clear_in_process_writer();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    occupy_socket_path_with_regular_file(root);

    let result = open_reader_client(root);

    clear_tier2_store_cache();
    assert!(
        result.is_err(),
        "socket present but unconnectable MUST fail closed for the reader too — \
         never silently fall through to a divergent tier-2 reader (bug #2896). \
         Got Ok(_)",
    );
}

/// Guard: with NO socket at all (the hermetic-test / standalone-CLI case), tier-2
/// remains the legitimate path and MUST succeed, so the fail-closed change does
/// not break the no-daemon workflow the whole test suite relies on.
#[test]
#[serial_test::serial(cognitive_memory)]
fn launch_clients_succeed_when_no_socket_present() {
    clear_in_process_writer();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // No file at socket_path_for(root): genuinely absent.

    let writer = launch_writer_client(root).expect("writer must open tier-2 when no socket exists");
    // A brand-new tier-2 store fronts zero facts.
    let stats = writer
        .ops()
        .get_statistics()
        .expect("stats on fresh tier-2 writer");
    assert_eq!(stats.semantic_count, 0);

    let reader = open_reader_client(root).expect("reader must open tier-2 when no socket exists");
    let facts = reader
        .ops()
        .search_facts("anything", 4, 0.0)
        .expect("search on fresh tier-2 reader");
    assert!(facts.is_empty());

    clear_tier2_store_cache();
}

/// A backend whose every abstract op fails, so a request that reaches it over the
/// socket produces an error the client must decode into `Err` (never a phantom
/// `Ok`). The `Ping` handshake is served by the protocol layer, not the backend,
/// so `connect` still succeeds — isolating the fail-closed guarantee to the op.
struct AlwaysErrBackend;

impl AlwaysErrBackend {
    fn boom(op: &str) -> SimardError {
        SimardError::RpcCallFailed {
            bridge: "memory-ipc".to_string(),
            method: op.to_string(),
            reason: "write-len: Broken pipe (os error 32)".to_string(),
        }
    }
}

impl CognitiveMemoryOps for AlwaysErrBackend {
    fn record_sensory(&self, _m: &str, _r: &str, _t: u64) -> SimardResult<String> {
        Err(Self::boom("record_sensory"))
    }
    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        Err(Self::boom("prune_expired_sensory"))
    }
    fn push_working(&self, _s: &str, _c: &str, _t: &str, _r: f64) -> SimardResult<String> {
        Err(Self::boom("push_working"))
    }
    fn get_working(&self, _t: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        Err(Self::boom("get_working"))
    }
    fn clear_working(&self, _t: &str) -> SimardResult<usize> {
        Err(Self::boom("clear_working"))
    }
    fn store_episode(
        &self,
        _c: &str,
        _s: &str,
        _m: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        Err(Self::boom("store_episode"))
    }
    fn consolidate_episodes(&self, _b: u32) -> SimardResult<Option<String>> {
        Err(Self::boom("consolidate_episodes"))
    }
    fn store_fact(
        &self,
        _c: &str,
        _co: &str,
        _cf: f64,
        _t: &[String],
        _s: &str,
    ) -> SimardResult<String> {
        Err(Self::boom("store_fact"))
    }
    fn search_facts(&self, _q: &str, _l: u32, _m: f64) -> SimardResult<Vec<CognitiveFact>> {
        Err(Self::boom("search_facts"))
    }
    fn store_procedure(&self, _n: &str, _s: &[String], _p: &[String]) -> SimardResult<String> {
        Err(Self::boom("store_procedure"))
    }
    fn recall_procedure(&self, _q: &str, _l: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        Err(Self::boom("recall_procedure"))
    }
    fn store_prospective(&self, _d: &str, _tc: &str, _a: &str, _p: i64) -> SimardResult<String> {
        Err(Self::boom("store_prospective"))
    }
    fn check_triggers(&self, _c: &str) -> SimardResult<Vec<CognitiveProspective>> {
        Err(Self::boom("check_triggers"))
    }
    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        Err(Self::boom("get_statistics"))
    }
    fn search_episodes_by_keywords(
        &self,
        _k: &[String],
        _l: u32,
    ) -> SimardResult<Vec<CognitiveEpisode>> {
        Err(Self::boom("search_episodes_by_keywords"))
    }
}

/// Guard (confirms the socket writer path stays fail-closed): a live daemon whose
/// backend op errors (broken pipe / backend failure) must surface as `Err` from
/// the writer op — never a swallowed `Ok`. This is the seam that keeps a routed
/// goal's write from being a phantom success across the IPC transport.
#[test]
#[serial_test::serial(cognitive_memory)]
fn writer_socket_op_error_surfaces_as_err_not_phantom_ok() {
    clear_in_process_writer();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let sock = socket_path_for(&root);

    let handle = spawn_server(sock.clone(), Arc::new(AlwaysErrBackend)).expect("spawn_server");
    // The listener binds on a background thread; wait for the socket to appear.
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // Connect succeeds (Ping is protocol-level), so we exercise tier-1 (socket).
    let writer = launch_writer_client(&root).expect("connect to live socket");
    let result = writer.ops().store_fact_with_caller_key(
        "goal-store:record:some-goal",
        "goal-store:record",
        "{\"slug\":\"some-goal\"}",
        1.0,
        &["goal-store".to_string()],
        "goal-store",
    );

    drop(handle);
    clear_tier2_store_cache();
    assert!(
        result.is_err(),
        "a memory-ipc write whose backend/transport fails MUST surface as Err, \
         never a swallowed Ok (bug #2896: phantom success across IPC). Got Ok(_)",
    );
}
