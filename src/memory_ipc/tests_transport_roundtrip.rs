//! End-to-end tests for the memory-IPC **Unix-socket transport** — the
//! [`RemoteCognitiveMemory`] client ([`super::client`]) and the
//! [`spawn_server`] / `serve_connection` / `dispatch` server
//! ([`super::server`]).
//!
//! Motivation (coverage gap): every other `memory_ipc` test exercises the
//! *in-process* launcher ladder (tier-0 `register_in_process_writer` and
//! tier-2 `shared_tier2_store`) via [`launch_writer_client`] /
//! [`open_reader_client`]. None of them drives the actual socket wire, so the
//! whole cross-process transport — the reason the module exists — was almost
//! untested (`client.rs` ~29 % / `server.rs` ~43 % line coverage). A protocol
//! break there would silently sever cross-process cognitive-memory access
//! (meeting REPL / engineer subprocess / dashboard ⇄ OODA daemon) with no
//! failing test, exactly the "hollow success" class the launcher's
//! no-read-only-fallback rule exists to prevent.
//!
//! These tests are **hermetic and behaviour-verifying**: each spins up a real
//! server thread bound to a `TempDir` socket, backed by a real
//! [`LibraryCognitiveMemory::in_memory()`] store (no disk, no network, no env,
//! no shared global state), then asserts that data written through the socket
//! *round-trips with its payload intact* — not merely that a call returns
//! `Ok`. They therefore need no `#[serial(cognitive_memory)]` key: they never
//! mutate a watched env var, construct `HermeticState`, or open the store at
//! the env-derived default path (see `test_support::serial_guard`).
//!
//! Scope: "every op" below means every operation in the IPC **request
//! protocol** (`MemoryRequest` / the `dispatch` arms), which is the subset of
//! [`CognitiveMemoryOps`] that actually crosses the socket. `CognitiveMemoryOps`
//! methods with no `MemoryRequest` variant (e.g. `store_fact_with_provenance`,
//! `graph_stats`) are served locally via trait defaults on the client and do
//! not traverse the wire; they are exercised against `SharedMemory` (the
//! in-process adapter) rather than the socket.

use std::sync::Arc;
use std::time::Duration;

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

use super::{RemoteCognitiveMemory, ServerHandle, spawn_server};

/// A live server (backed by an in-memory store) plus a connected client, all
/// rooted in one `TempDir` whose lifetime keeps the socket path valid.
struct Fixture {
    client: RemoteCognitiveMemory,
    _handle: ServerHandle,
    _dir: tempfile::TempDir,
}

/// Spawn a server on a fresh `TempDir` socket backed by `backend`, wait for it
/// to start accepting, and return a connected client.
fn fixture_with_backend(backend: Arc<dyn CognitiveMemoryOps>) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("memory.sock");
    let handle = spawn_server(sock.clone(), backend).expect("spawn_server");

    // The listener binds on a background thread; poll until the socket file is
    // present before connecting (matches the existing top-level round-trip
    // test's start-up handshake).
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let client = RemoteCognitiveMemory::connect(&sock).expect("client connect + Ping handshake");
    Fixture {
        client,
        _handle: handle,
        _dir: dir,
    }
}

/// The common case: a real in-memory cognitive store behind the socket.
fn in_memory_fixture() -> Fixture {
    let backend: Arc<dyn CognitiveMemoryOps> =
        Arc::new(LibraryCognitiveMemory::in_memory().expect("in-memory store"));
    fixture_with_backend(backend)
}

// ---------------------------------------------------------------------------
// Per-operation round-trips: each asserts the *payload* survives the wire, so
// a broken serde tag / dispatch arm / response-variant match fails the test.
// ---------------------------------------------------------------------------

#[test]
fn sensory_record_returns_id_and_fresh_record_is_not_pruned() {
    let fx = in_memory_fixture();

    // A long TTL means the record is *not* expired, so a prune immediately
    // afterwards must reclaim nothing. This pins both the RecordSensory→Id and
    // PruneExpiredSensory→Count wire mappings AND the "don't drop live data"
    // behaviour — a prune that returned >0 here would be silently destroying a
    // still-valid sensory memory.
    let id = fx
        .client
        .record_sensory("text", "operator said hello", 3_600)
        .expect("record_sensory over socket");
    assert!(
        !id.is_empty(),
        "record_sensory must return a non-empty node id"
    );

    let pruned = fx
        .client
        .prune_expired_sensory()
        .expect("prune_expired_sensory over socket");
    assert_eq!(
        pruned, 0,
        "a sensory record with a 1-hour TTL must not be pruned immediately"
    );
}

#[test]
fn working_memory_push_get_clear_lifecycle_over_socket() {
    let fx = in_memory_fixture();
    let task = "task-abc";

    let node_id = fx
        .client
        .push_working("plan", "draft the migration", task, 0.75)
        .expect("push_working over socket");
    assert!(!node_id.is_empty(), "push_working must return a node id");

    let slots = fx
        .client
        .get_working(task)
        .expect("get_working over socket");
    assert_eq!(
        slots.len(),
        1,
        "exactly the one pushed slot must be visible"
    );
    let slot = &slots[0];
    assert_eq!(
        slot.node_id, node_id,
        "the retrieved slot must be the one push_working returned (id ties record to write)"
    );
    assert_eq!(
        slot.content, "draft the migration",
        "content must survive the wire"
    );
    assert_eq!(slot.slot_type, "plan", "slot_type must survive the wire");
    assert_eq!(slot.task_id, task, "task_id must survive the wire");
    assert!(
        (slot.relevance - 0.75).abs() < 1e-9,
        "relevance f64 must survive the wire; got {}",
        slot.relevance
    );

    let cleared = fx
        .client
        .clear_working(task)
        .expect("clear_working over socket");
    assert_eq!(cleared, 1, "clear_working must report the one removed slot");
    assert!(
        fx.client
            .get_working(task)
            .expect("get_working after clear")
            .is_empty(),
        "the task's working set must be empty after clear_working"
    );
}

#[test]
fn episode_store_search_and_consolidate_over_socket() {
    let fx = in_memory_fixture();

    let e1 = fx
        .client
        .store_episode("engineer merged PR 42 to fix the socket race", "test", None)
        .expect("store_episode #1");
    let e2 = fx
        .client
        .store_episode(
            "engineer merged PR 43 tightening the socket timeout",
            "test",
            None,
        )
        .expect("store_episode #2");
    // A decoy episode WITHOUT the "socket" keyword: it must NOT come back from
    // the keyword search, so the search is proven to actually filter on the
    // wire (not just "return everything").
    let decoy = fx
        .client
        .store_episode("engineer rewrote the onboarding README", "test", None)
        .expect("store_episode decoy");
    assert!(!e1.is_empty() && !e2.is_empty(), "episodes must get ids");
    assert_ne!(e1, e2, "distinct episodes must get distinct ids");

    // Keyword recall must reach the backend across the wire (this method has a
    // real default-less client impl; a no-op default would return nothing).
    let hits = fx
        .client
        .search_episodes_by_keywords(&["socket".to_string()], 10)
        .expect("search_episodes_by_keywords over socket");
    assert!(
        hits.iter().any(|e: &CognitiveEpisode| e.node_id == e1)
            && hits.iter().any(|e| e.node_id == e2),
        "both 'socket' episodes must be recalled by keyword; got {} hits",
        hits.len()
    );
    assert!(
        !hits.iter().any(|e| e.node_id == decoy),
        "the non-'socket' decoy episode must NOT be returned — proves the keyword \
         filter is applied server-side, not ignored"
    );

    // With three un-compressed episodes available, a batch-2 consolidation must
    // produce a non-empty summary node id (exercising the MaybeId variant).
    let consolidated = fx
        .client
        .consolidate_episodes(2)
        .expect("consolidate_episodes over socket");
    let con_id = consolidated.expect("consolidating 2 available episodes must yield Some(id)");
    assert!(
        !con_id.is_empty(),
        "the consolidated node id must be non-empty"
    );
}

#[test]
fn fact_store_search_and_statistics_over_socket() {
    let fx = in_memory_fixture();

    let stats_before = fx.client.get_statistics().expect("get_statistics (before)");
    assert_eq!(
        stats_before.semantic_count, 0,
        "a fresh store must report zero semantic facts"
    );

    let id = fx
        .client
        .store_fact(
            "gravity",
            "objects fall at 9.8 m/s^2",
            0.9,
            &["physics".to_string(), "mechanics".to_string()],
            "src-newton",
        )
        .expect("store_fact over socket");
    assert!(!id.is_empty(), "store_fact must return a node id");

    // Decoy 1: a query non-match (different concept). Decoy 2: a query match but
    // LOW confidence. Together they let the two assertions below prove that BOTH
    // the query filter AND the min_confidence filter are applied server-side —
    // not that search_facts is a "return everything" stub.
    fx.client
        .store_fact("gardening", "water tomatoes weekly", 0.9, &[], "src-decoy")
        .expect("store decoy fact (query non-match)");
    fx.client
        .store_fact("gravity", "gravity is a myth", 0.3, &[], "src-myth")
        .expect("store decoy fact (low confidence)");

    // (a) query filter: searching "gravity" at min_confidence 0.0 returns both
    // gravity facts but NOT the gardening fact.
    let by_query: Vec<CognitiveFact> = fx
        .client
        .search_facts("gravity", 10, 0.0)
        .expect("search_facts over socket");
    let real = by_query
        .iter()
        .find(|f| f.content == "objects fall at 9.8 m/s^2")
        .expect("the stored fact's content must round-trip through the socket");
    assert!(
        (real.confidence - 0.9).abs() < 1e-9,
        "the fact's confidence must round-trip; got {}",
        real.confidence
    );
    assert!(
        !by_query.iter().any(|f| f.concept == "gardening"),
        "the query filter must exclude the unrelated 'gardening' fact over the wire"
    );

    // (b) confidence filter: raising min_confidence to 0.5 drops the 0.3 "myth"
    // fact while keeping the 0.9 fact.
    let by_conf: Vec<CognitiveFact> = fx
        .client
        .search_facts("gravity", 10, 0.5)
        .expect("search_facts (min_confidence)");
    assert!(
        by_conf
            .iter()
            .any(|f| f.content == "objects fall at 9.8 m/s^2"),
        "the high-confidence fact must survive the min_confidence filter"
    );
    assert!(
        !by_conf.iter().any(|f| f.content == "gravity is a myth"),
        "the min_confidence filter must drop the 0.3 fact over the wire"
    );

    let stats_after = fx.client.get_statistics().expect("get_statistics (after)");
    assert_eq!(
        stats_after.semantic_count, 3,
        "statistics fetched over the socket must reflect all three stored facts"
    );
}

#[test]
fn recall_facts_ranked_forwards_ranked_recall_over_socket() {
    // Regression: on the production daemon path OODA memory is a
    // `RemoteCognitiveMemory` socket client. Before this fix the client had no
    // `recall_facts_ranked` override, so it silently fell back to the trait
    // default → `search_facts` RPC → the server's word-boundary-GATED keyword
    // search. That discarded the flagship six-signal, phase-weighted ranked
    // recall (#2329) AND its `recall_precision_at_k` metric on the primary
    // production path — a hollow success invisible to callers.
    //
    // The behavioural distinguisher: `search_facts` GATES out a fact that does
    // not share a query word, whereas library `recall_facts_ranked` RANKS all
    // candidate facts (ungated) and returns them up to `limit`. So a fact that
    // does NOT match the query word must be:
    //   * ABSENT from `search_facts` (proven by the query-filter test above), and
    //   * PRESENT in `recall_facts_ranked` — which can only happen if the client
    //     forwarded the call to the server's library ranked recall rather than
    //     degrading to gated `search_facts`.
    use crate::cognitive_memory::RecallWeightSet;

    let fx = in_memory_fixture();

    // A query-matching fact and a query-NON-matching fact (no "gravity" word).
    fx.client
        .store_fact(
            "gravity",
            "objects fall at nine point eight",
            0.9,
            &[],
            "src-a",
        )
        .expect("store query-matching fact over socket");
    fx.client
        .store_fact("gardening", "water tomatoes weekly", 0.9, &[], "src-b")
        .expect("store query-non-matching fact over socket");

    // Contrast: gated search_facts excludes the non-matching 'gardening' fact.
    let searched = fx
        .client
        .search_facts("gravity", 10, 0.0)
        .expect("search_facts over socket");
    assert!(
        !searched.iter().any(|f| f.concept == "gardening"),
        "search_facts must GATE OUT the query-non-matching 'gardening' fact \
         (this is the behaviour the buggy ranked-recall fallback inherited)"
    );

    // Ranked recall over the socket, with deliberately NON-default weights so
    // the `RecallWeightSet` payload's real field values (not just its Default)
    // must serialize/deserialize across the wire. `recall_facts_ranked` ranks
    // ALL candidate facts, so the 'gardening' fact — gated out of search_facts —
    // must appear here. If the client had degraded to gated search_facts it
    // would be absent and this assertion would fail.
    let weights = RecallWeightSet {
        text_relevance: 0.2,
        confidence: 0.9,
        importance: 0.1,
        recency: 0.7,
        usage: 0.3,
        graph: 0.5,
    };
    let ranked = fx
        .client
        .recall_facts_ranked("gravity", 10, 0.0, weights)
        .expect("recall_facts_ranked over socket");
    assert!(
        ranked.iter().any(|f| f.concept == "gravity"),
        "ranked recall must include the query-matching fact"
    );
    assert!(
        ranked.iter().any(|f| f.concept == "gardening"),
        "ranked recall over the socket must FORWARD to the library's six-signal \
         ranked recall (which returns the query-non-matching 'gardening' fact) \
         rather than degrade to gated search_facts — see #2329 / #2627 lineage"
    );
}

#[test]
fn procedure_store_and_recall_over_socket() {
    let fx = in_memory_fixture();

    let steps = vec![
        "run cargo test".to_string(),
        "open a PR".to_string(),
        "merge when green".to_string(),
    ];
    let prereqs = vec!["clean worktree".to_string()];
    let id = fx
        .client
        .store_procedure("ship_change", &steps, &prereqs)
        .expect("store_procedure over socket");
    assert!(!id.is_empty(), "store_procedure must return a node id");

    // Decoy procedure whose name does not contain the query token, so a correct
    // (server-side) name filter excludes it — proving recall_procedure filters
    // rather than returning every stored procedure.
    fx.client
        .store_procedure("rollback_release", &["revert".to_string()], &[])
        .expect("store decoy procedure");

    let recalled: Vec<CognitiveProcedure> = fx
        .client
        .recall_procedure("ship_change", 10)
        .expect("recall_procedure over socket");
    let proc = recalled
        .iter()
        .find(|p| p.name == "ship_change")
        .expect("the stored procedure must be recalled by name");
    assert_eq!(
        proc.node_id, id,
        "the recalled procedure must be the one store_procedure returned"
    );
    assert_eq!(
        proc.steps, steps,
        "procedure steps must round-trip through the socket intact"
    );
    assert_eq!(
        proc.prerequisites, prereqs,
        "procedure prerequisites must round-trip through the socket intact"
    );
    assert!(
        !recalled.iter().any(|p| p.name == "rollback_release"),
        "recall_procedure must exclude the non-matching decoy procedure over the wire"
    );
}

#[test]
fn list_all_episodes_and_prospective_enumerate_over_socket() {
    // Issue #2627: the dashboard Memory-tab graph reads live per-item nodes for
    // episodes and prospective memories through `list_all_episodes` /
    // `list_all_prospective`. Both have empty trait defaults, so unless the
    // socket client FORWARDS them (and the server dispatches them) a reader on
    // the daemon-socket tier would silently collapse both types to their type
    // hub — exactly the regression this additive forward prevents.
    let fx = in_memory_fixture();

    let ep = fx
        .client
        .store_episode("engineer restored the memory tab graph", "test", None)
        .expect("store_episode over socket");
    let pr = fx
        .client
        .store_prospective(
            "watch for memory-graph regressions",
            "when a memory socket test fails",
            "page the on-call engineer",
            7,
        )
        .expect("store_prospective over socket");

    let episodes = fx
        .client
        .list_all_episodes(50)
        .expect("list_all_episodes over socket");
    assert!(
        episodes.iter().any(|e: &CognitiveEpisode| e.node_id == ep),
        "the stored episode must enumerate over the socket (a missing forward \
         would return the empty trait default); got {} episodes",
        episodes.len()
    );

    let prospective = fx
        .client
        .list_all_prospective(50)
        .expect("list_all_prospective over socket");
    assert!(
        prospective.iter().any(|p| p.node_id == pr),
        "the stored prospective memory must enumerate over the socket; got {} \
         prospective",
        prospective.len()
    );
}

#[test]
fn prospective_store_trigger_and_resolve_over_socket() {
    let fx = in_memory_fixture();

    let id = fx
        .client
        .store_prospective(
            "notify on deploy",
            "deploy production",
            "send the release email",
            5,
        )
        .expect("store_prospective over socket");
    assert!(!id.is_empty(), "store_prospective must return a node id");

    // A decoy prospective whose trigger shares NO tokens with the content below,
    // so it must not fire — proving check_triggers actually matches on the wire
    // rather than returning every prospective.
    let decoy = fx
        .client
        .store_prospective("nightly backup", "backup nightly", "run pg_dump", 1)
        .expect("store decoy prospective");

    // check_triggers uses keyword-overlap matching; content sharing the trigger
    // tokens must fire the prospective and return it over the wire.
    let fired: Vec<CognitiveProspective> = fx
        .client
        .check_triggers("it is time to deploy production now")
        .expect("check_triggers over socket");
    let matched = fired
        .iter()
        .find(|p| p.node_id == id)
        .expect("the matching prospective must fire and return over the socket");
    assert_eq!(
        matched.action_on_trigger, "send the release email",
        "the fired prospective's action must round-trip intact"
    );
    assert!(
        !fired.iter().any(|p| p.node_id == decoy),
        "the non-matching decoy prospective must NOT fire — proves keyword \
         matching is applied server-side"
    );

    // Resolving it must be acknowledged (Ack response variant → Ok(())).
    fx.client
        .resolve_prospective(&id)
        .expect("resolve_prospective over socket must be acknowledged");
}

/// The #122 dashboard fix must work over the Unix-socket transport too: the
/// dashboard reads through the IPC client (tier-1), so
/// `list_prospective_by_trigger` has to round-trip end-to-end (client request →
/// server dispatch → backend trigger-scoped query) rather than silently fall
/// back to the empty trait default (the exact silent-empty-read hazard that
/// left the dashboard showing 0 ideas).
#[test]
fn list_prospective_by_trigger_round_trips_over_socket() {
    let fx = in_memory_fixture();

    // A non-matching decoy under a different trigger — must NOT come back,
    // proving the trigger filter is applied server-side, not on the client.
    let decoy = fx
        .client
        .store_prospective("decoy", "unrelated-trigger", "{}", 9)
        .expect("store decoy over socket");

    // A handful of matching nodes under the creative-idea trigger.
    let mut matching = Vec::new();
    for i in 0i64..5 {
        matching.push(
            fx.client
                .store_prospective(&format!("creative {i}"), "creative-idea", "{}", i)
                .expect("store matching over socket"),
        );
    }

    let got: Vec<CognitiveProspective> = fx
        .client
        .list_prospective_by_trigger("creative-idea", 512)
        .expect("list_prospective_by_trigger over socket");

    for id in &matching {
        assert!(
            got.iter().any(|p| &p.node_id == id),
            "matching prospective {id} must round-trip back over the socket, not \
             be dropped to the empty default"
        );
    }
    assert!(
        !got.iter().any(|p| p.node_id == decoy),
        "the decoy under a different trigger must be filtered server-side"
    );
    assert_eq!(
        got.len(),
        matching.len(),
        "exactly the trigger-matching nodes cross the wire"
    );
}

// ---------------------------------------------------------------------------
// Error paths.
// ---------------------------------------------------------------------------

#[test]
fn connect_returns_spawn_error_when_socket_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("nope.sock");

    let err = match RemoteCognitiveMemory::connect(&missing) {
        Ok(_) => panic!("connecting to a non-existent socket must fail, not succeed"),
        Err(e) => e,
    };
    match err {
        SimardError::RpcSpawnFailed { endpoint, reason } => {
            assert_eq!(endpoint, "memory-ipc-client");
            assert!(
                reason.contains("not present"),
                "error must explain the socket is absent; got: {reason}"
            );
        }
        other => panic!("expected RpcSpawnFailed for a missing socket, got {other:?}"),
    }
}

/// Backend whose every abstract op fails, so a call that reaches it produces a
/// `MemoryResponse::Error`. Used to prove the server encodes backend errors and
/// the client decodes them into a typed `RpcCallFailed` carrying the *method
/// name* — the seam that stops an IPC store from silently "succeeding".
struct AlwaysErrBackend;

impl AlwaysErrBackend {
    fn boom(op: &str) -> SimardError {
        SimardError::RpcCallFailed {
            endpoint: "test-backend".into(),
            method: op.into(),
            reason: "synthetic backend failure".into(),
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
    fn recall_facts_ranked(
        &self,
        _q: &str,
        _l: u32,
        _m: f64,
        _w: crate::cognitive_memory::RecallWeightSet,
    ) -> SimardResult<Vec<CognitiveFact>> {
        // Overridden (not the trait default, which would delegate to
        // `search_facts`) so the error-encoding test pins the NEW
        // `RecallFactsRanked` server dispatch arm and client decode arm — a
        // default delegation would mis-attribute the failure to `search_facts`.
        Err(Self::boom("recall_facts_ranked"))
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
    // These two are trait-default methods, but they ARE part of the IPC request
    // surface (ResolveProspective / SearchEpisodesByKeywords dispatch arms).
    // Override them to Err so the error-encoding test can pin their server Err
    // arm and client decode arm too (a default Ok would mask a regression).
    fn resolve_prospective(&self, _n: &str) -> SimardResult<()> {
        Err(Self::boom("resolve_prospective"))
    }
    fn search_episodes_by_keywords(
        &self,
        _k: &[String],
        _l: u32,
    ) -> SimardResult<Vec<CognitiveEpisode>> {
        Err(Self::boom("search_episodes_by_keywords"))
    }
}

/// Assert a client result is the typed `RpcCallFailed` produced when the server
/// encoded a backend error and the client decoded it — attributed to `method`
/// and carrying the backend's message.
fn assert_backend_err<T: std::fmt::Debug>(result: SimardResult<T>, method: &str) {
    match result {
        Ok(v) => panic!(
            "{method}: expected a backend error, but got Ok({v:?}) — a failing IPC op must never silently succeed"
        ),
        Err(SimardError::RpcCallFailed {
            endpoint,
            method: got,
            reason,
        }) => {
            assert_eq!(endpoint, "memory-ipc", "{method}: endpoint label");
            assert_eq!(
                got, method,
                "the failure must be attributed to the method the client called"
            );
            assert!(
                reason.contains("synthetic backend failure"),
                "{method}: the backend's error message must be carried back across the socket; got: {reason}"
            );
        }
        Err(other) => panic!("{method}: expected RpcCallFailed, got {other:?}"),
    }
}

#[test]
fn every_op_encodes_backend_errors_as_rpc_call_failed() {
    // The Ping handshake does not touch the backend, so connect still succeeds;
    // then EVERY abstract op that reaches the failing backend must come back as
    // a typed `RpcCallFailed` naming that op. This exercises the server's
    // per-request `Err(..) => MemoryResponse::Error(..)` dispatch arm and the
    // client's `other => Err(unexpected(..))` decode arm for the whole surface
    // — the seam that prevents a cross-process write from "hollow-succeeding".
    let fx = fixture_with_backend(Arc::new(AlwaysErrBackend));
    let c = &fx.client;

    assert_backend_err(c.record_sensory("m", "d", 1), "record_sensory");
    assert_backend_err(c.prune_expired_sensory(), "prune_expired_sensory");
    assert_backend_err(c.push_working("s", "c", "t", 0.1), "push_working");
    assert_backend_err(c.get_working("t"), "get_working");
    assert_backend_err(c.clear_working("t"), "clear_working");
    assert_backend_err(c.store_episode("c", "s", None), "store_episode");
    assert_backend_err(c.consolidate_episodes(2), "consolidate_episodes");
    assert_backend_err(c.store_fact("c", "v", 1.0, &[], "s"), "store_fact");
    assert_backend_err(c.search_facts("q", 1, 0.0), "search_facts");
    assert_backend_err(
        c.recall_facts_ranked(
            "q",
            1,
            0.0,
            crate::cognitive_memory::RecallWeightSet::default(),
        ),
        "recall_facts_ranked",
    );
    assert_backend_err(c.store_procedure("n", &[], &[]), "store_procedure");
    assert_backend_err(c.recall_procedure("q", 1), "recall_procedure");
    assert_backend_err(c.store_prospective("d", "t", "a", 0), "store_prospective");
    assert_backend_err(c.check_triggers("c"), "check_triggers");
    assert_backend_err(c.get_statistics(), "get_statistics");
    assert_backend_err(c.resolve_prospective("id"), "resolve_prospective");
    assert_backend_err(
        c.search_episodes_by_keywords(&["x".to_string()], 1),
        "search_episodes_by_keywords",
    );
}

#[test]
fn client_socket_path_accessor_reports_the_connected_path() {
    let fx = in_memory_fixture();
    assert!(
        fx.client.socket_path().ends_with("memory.sock"),
        "socket_path() must report the path the client connected to; got {}",
        fx.client.socket_path().display()
    );
}

#[test]
fn one_connection_serves_many_sequential_requests() {
    // serve_connection loops reading framed requests until EOF; issue several
    // ops on the same client (same UnixStream) and confirm each is answered,
    // proving the per-connection request loop — not just the first frame — works.
    let fx = in_memory_fixture();
    for i in 0..5 {
        let id = fx
            .client
            .store_fact(
                &format!("concept-{i}"),
                &format!("value-{i}"),
                0.5,
                &[],
                "loop-src",
            )
            .unwrap_or_else(|e| panic!("store_fact #{i} on the shared connection failed: {e}"));
        assert!(!id.is_empty(), "each looped store_fact must return an id");
    }
    let stats = fx.client.get_statistics().expect("final get_statistics");
    assert_eq!(
        stats.semantic_count, 5,
        "all five sequential writes on one connection must have landed"
    );
}

#[test]
fn server_handle_drop_removes_the_socket_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("memory.sock");
    let backend: Arc<dyn CognitiveMemoryOps> =
        Arc::new(LibraryCognitiveMemory::in_memory().expect("in-memory store"));
    let handle = spawn_server(sock.clone(), backend).expect("spawn_server");
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        sock.exists(),
        "precondition: the server must have bound the socket"
    );

    drop(handle);
    assert!(
        !sock.exists(),
        "dropping the ServerHandle must unlink the socket file so a later daemon can rebind it"
    );
}

// ---------------------------------------------------------------------------
// Stale-lock reaping: the live-owner branch (the existing top-level test only
// covers the absent-file and empty-file/flock branches).
// ---------------------------------------------------------------------------

#[test]
fn reap_stale_open_lock_keeps_a_lock_owned_by_a_live_process() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lock = dir.path().join("cognitive_memory.ladybug.open.lock");
    // Record *this* process's pid: it is provably alive, so the reaper must
    // refuse to remove the lock (removing a live daemon's lock would let a
    // second writer open the store concurrently and corrupt it).
    std::fs::write(&lock, std::process::id().to_string()).expect("seed lock file");

    let reaped = super::reap_stale_open_lock(dir.path()).expect("reap must not error");
    assert!(!reaped, "a lock owned by a live pid must NOT be reaped");
    assert!(
        lock.exists(),
        "the live-owned lock file must be left in place"
    );
}

// ---------------------------------------------------------------------------
// Server robustness: a malformed request frame must not take the server down.
// ---------------------------------------------------------------------------

#[test]
fn malformed_request_frame_does_not_kill_the_server() {
    use std::os::unix::net::UnixStream;

    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("memory.sock");
    let backend: Arc<dyn CognitiveMemoryOps> =
        Arc::new(LibraryCognitiveMemory::in_memory().expect("in-memory store"));
    let _handle = spawn_server(sock.clone(), backend).expect("spawn_server");
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // Send a well-framed but NON-JSON payload on a raw connection. The server's
    // `serve_connection` must fail to parse it and drop *this* connection
    // (serialize-request/parse-request error path) without aborting the accept
    // loop.
    {
        let mut raw = UnixStream::connect(&sock).expect("raw connect");
        super::write_frame(&mut raw, b"this is not valid json").expect("write malformed frame");
        // Give the server's connection handler a moment to read + reject it.
        std::thread::sleep(Duration::from_millis(50));
    }

    // The server must still be alive: a fresh, well-behaved client connects and
    // completes a real round-trip.
    let client = RemoteCognitiveMemory::connect(&sock)
        .expect("server must still accept new clients after a malformed frame");
    let id = client
        .store_fact("resilience", "one bad frame is not fatal", 1.0, &[], "src")
        .expect("a healthy client must still be served after a peer sent garbage");
    assert!(
        !id.is_empty(),
        "the post-malformed-frame round-trip must succeed"
    );
}

// ---------------------------------------------------------------------------
// SharedMemory adapter: forwards the whole CognitiveMemoryOps surface to the
// wrapped store. A missing forward is a silent-empty-read hazard (issue #2320 /
// #2331) — the daemon (tier-0) and every tier-2 client read through this.
// ---------------------------------------------------------------------------

#[test]
fn shared_memory_forwards_the_whole_ops_surface_to_the_inner_store() {
    use super::SharedMemory;

    let inner: Arc<dyn CognitiveMemoryOps> =
        Arc::new(LibraryCognitiveMemory::in_memory().expect("in-memory store"));
    let shared = SharedMemory(Arc::clone(&inner));

    // Passthrough scalars.
    assert!(
        !shared.is_read_only(),
        "a writable inner store must forward is_read_only=false"
    );

    // Sensory.
    let sid = shared
        .record_sensory("text", "hi", 3_600)
        .expect("record_sensory");
    assert!(!sid.is_empty());
    assert_eq!(
        shared.prune_expired_sensory().expect("prune"),
        0,
        "live sensory not pruned"
    );

    // Working.
    let wid = shared
        .push_working("plan", "step one", "task-1", 0.5)
        .expect("push_working");
    assert!(!wid.is_empty());
    assert_eq!(shared.get_working("task-1").expect("get_working").len(), 1);
    assert_eq!(shared.clear_working("task-1").expect("clear_working"), 1);

    // Episodic (+ the default-less recall/distill methods whose trait defaults
    // are empty no-ops — a missing forward would return nothing).
    let ep = shared
        .store_episode("engineer fixed the shared-memory forward", "test", None)
        .expect("store_episode");
    assert!(
        shared
            .list_all_episodes(10)
            .expect("list_all_episodes")
            .iter()
            .any(|e| e.node_id == ep)
    );
    assert!(
        shared
            .search_episodes_by_keywords(&["shared-memory".to_string()], 10)
            .expect("search_episodes_by_keywords")
            .iter()
            .any(|e| e.node_id == ep)
    );
    assert!(
        shared
            .list_undistilled_episodes(10)
            .expect("list_undistilled_episodes")
            .iter()
            .any(|e| e.node_id == ep)
    );
    assert!(
        shared
            .search_episodes_starting_with("engineer fixed", 10)
            .expect("search_episodes_starting_with")
            .iter()
            .any(|(content, _)| content == "engineer fixed the shared-memory forward"),
        "search_episodes_starting_with must forward and return the matching episode's content"
    );
    shared
        .mark_episode_distilled(&ep)
        .expect("mark_episode_distilled");
    assert!(
        !shared
            .list_undistilled_episodes(10)
            .expect("list_undistilled after mark")
            .iter()
            .any(|e| e.node_id == ep),
        "mark_episode_distilled must forward: the episode leaves the undistilled set"
    );

    // Semantic facts (+ provenance + ranked + caller-key dedup + pruning).
    let f1 = shared
        .store_fact("concept-a", "value-a", 0.9, &["t".to_string()], "src-a")
        .expect("store_fact");
    assert!(
        shared
            .search_facts("concept-a", 10, 0.0)
            .expect("search_facts")
            .iter()
            .any(|f| f.node_id == f1 || f.concept == "concept-a")
    );
    assert!(
        !shared
            .recall_facts_ranked(
                "concept-a",
                10,
                0.0,
                crate::cognitive_memory::RecallWeightSet::default()
            )
            .expect("recall_facts_ranked")
            .is_empty(),
        "recall_facts_ranked must forward and find the stored fact"
    );
    let prov_fact = shared
        .store_fact_with_provenance(
            "lesson",
            "keep it green",
            0.8,
            "src-p",
            None,
            None,
            std::slice::from_ref(&ep),
        )
        .expect("store_fact_with_provenance");
    assert!(
        !shared
            .episodes_for_fact(&prov_fact)
            .expect("episodes_for_fact")
            .is_empty(),
        "episodes_for_fact must forward and surface the provenance edge"
    );
    // Caller-key dedup: storing the SAME key with the SAME content must REUSE
    // the node (identical id), which the trait default (delegating to store_fact)
    // would NOT do — so this proves the real caller-key path is forwarded.
    let ck1 = shared
        .store_fact_with_caller_key("k1", "dedup-concept", "v", 0.7, &[], "src-k")
        .expect("store_fact_with_caller_key #1");
    let ck2 = shared
        .store_fact_with_caller_key("k1", "dedup-concept", "v", 0.7, &[], "src-k")
        .expect("store_fact_with_caller_key #2");
    assert!(!ck1.is_empty());
    assert_eq!(
        ck1, ck2,
        "re-storing an identical caller-key fact must reuse the same node (dedup forwarded)"
    );
    // No caller-key fact was re-stored with CHANGED content and nothing was
    // superseded, so prune_superseded must reclaim exactly zero — a concrete
    // assertion (not just "did not error") that also proves it forwards.
    assert_eq!(
        shared
            .prune_superseded()
            .expect("prune_superseded must forward"),
        0,
        "with no superseded facts, prune_superseded must reclaim nothing"
    );
    // graph_stats must forward the real provenance edge (not the zeroed default).
    assert!(
        shared
            .graph_stats()
            .expect("graph_stats")
            .derives_from_edges
            >= 1,
        "graph_stats must forward the DERIVES_FROM edge from the provenance fact"
    );

    // Procedural.
    let pid = shared
        .store_procedure("deploy", &["a".to_string(), "b".to_string()], &[])
        .expect("store_procedure");
    assert!(!pid.is_empty());
    assert!(
        shared
            .recall_procedure("deploy", 10)
            .expect("recall_procedure")
            .iter()
            .any(|p| p.name == "deploy")
    );
    assert!(
        shared.procedure_exists("deploy").expect("procedure_exists"),
        "procedure_exists must forward"
    );
    let pp = shared
        .store_procedure_with_provenance(
            "distilled-proc",
            &["x".to_string()],
            &[],
            std::slice::from_ref(&ep),
        )
        .expect("store_procedure_with_provenance");
    assert!(!pp.is_empty());
    // The provenance forward must create a procedure->episode DERIVES_FROM edge;
    // the trait default (delegating to store_procedure) would create none.
    assert!(
        shared
            .graph_stats()
            .expect("graph_stats after procedure provenance")
            .procedure_derives_from_edges
            >= 1,
        "store_procedure_with_provenance must forward and record a procedure provenance edge"
    );

    // Prospective.
    let pr = shared
        .store_prospective("notify", "release now", "email", 3)
        .expect("store_prospective");
    assert!(
        shared
            .list_all_prospective(10)
            .expect("list_all_prospective")
            .iter()
            .any(|p| p.node_id == pr),
        "list_all_prospective must forward"
    );
    assert!(
        shared
            .check_triggers("release now please")
            .expect("check_triggers")
            .iter()
            .any(|p| p.node_id == pr),
        "check_triggers must forward"
    );
    shared
        .resolve_prospective(&pr)
        .expect("resolve_prospective must forward");

    // Statistics reflect the writes made through the shared adapter.
    let stats = shared.get_statistics().expect("get_statistics");
    assert!(
        stats.semantic_count >= 2,
        "stats must forward and reflect the facts written via SharedMemory"
    );
    assert!(
        stats.procedural_count >= 2,
        "stats must reflect the procedures written via SharedMemory"
    );

    // checkpoint forwards (durability flush) without error.
    shared.checkpoint().expect("checkpoint must forward");
}

/// Tier-0 (in-process) path: the daemon's own dashboard reader goes through
/// [`SharedMemory`], so it must forward `list_prospective_by_trigger` to the
/// inner store rather than returning the empty trait default — the same
/// silent-empty-read hazard (#2320 / #2331) that a missing forward would
/// reintroduce, now on the #122 creative-ideas read path.
#[test]
fn shared_memory_forwards_list_prospective_by_trigger() {
    use super::SharedMemory;

    let inner: Arc<dyn CognitiveMemoryOps> =
        Arc::new(LibraryCognitiveMemory::in_memory().expect("in-memory store"));
    let shared = SharedMemory(Arc::clone(&inner));

    let decoy = shared
        .store_prospective("decoy", "unrelated-trigger", "{}", 1)
        .expect("store decoy");
    let target = shared
        .store_prospective("idea", "creative-idea", "{}", 7)
        .expect("store target");

    let got = shared
        .list_prospective_by_trigger("creative-idea", 512)
        .expect("list_prospective_by_trigger must forward");

    assert!(
        got.iter().any(|p| p.node_id == target),
        "SharedMemory must forward list_prospective_by_trigger to the inner store"
    );
    assert!(
        !got.iter().any(|p| p.node_id == decoy),
        "the forwarded call must preserve the inner store's trigger filtering"
    );
}
