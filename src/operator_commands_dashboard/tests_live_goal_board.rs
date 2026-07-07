//! Failing TDD tests (issue #2922, Step 7) for the dashboard's LIVE goal-board
//! read.
//!
//! Before #2922 the Goals tab (`GET /api/goals` → `goals_at`) and the Memory
//! tab's Goal-Records tile (`GET /api/memory` → `memory_metrics`) read ONLY the
//! `goal-board:snapshot` fact — a cache the daemon rewrites ~once per OODA cycle
//! (~5 min). A promoted creative-idea Proposed goal persists a `goal-store:record`
//! that the snapshot does not contain, so it did not appear on the board until
//! the next cycle. This is the staleness #2922 fixes.
//!
//! The fix introduces a single fail-closed live builder that unions the snapshot
//! base with a LIVE `CognitiveMemoryGoalStore` overlay:
//!
//! ```ignore
//! pub(crate) fn dashboard_live_goal_board(state_root: &Path) -> SimardResult<GoalBoard>;
//! ```
//!
//! and repoints `goals_at` / `memory_metrics` at it. These tests reference
//! `dashboard_live_goal_board` (which does not exist yet) and assert the
//! repointed, fail-closed behaviour of the handlers — the intended TDD red
//! (compile-fail) state until the implementation lands.
//!
//! Hermetic: a shared in-process `LibraryCognitiveMemory` writer (tier 0) is
//! registered for a `TempDir` state root so both the snapshot base read and the
//! goal-store overlay resolve to the SAME live store, exactly like production.
//! The fail-closed tests instead register a fault-injecting backend so a
//! transport error surfaces. Registration mutates process-global state, so every
//! test is `#[serial_test::serial(cognitive_memory)]` and clears on exit.

use std::path::Path;
use std::sync::Arc;

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::error::{SimardError, SimardResult};
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, save_goal_board};
use crate::goals::{CognitiveMemoryGoalStore, GoalRecord, GoalStatus, GoalStore, goal_slug};
use crate::memory_cognitive::{
    CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};
use crate::memory_ipc::{clear_in_process_writer, register_in_process_writer};
use crate::session::{SessionId, SessionPhase};
use crate::test_support::HermeticState;

use super::dashboard_live_goal_board;
use super::goals::goals_at;
use super::metrics::memory_metrics;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Holds the single shared in-process cognitive-memory writer for a test and
/// clears the global registration on drop (panic-safe). Mirrors the tier-0
/// wiring the daemon / dashboard use in production.
struct SharedMem {
    writer: Arc<dyn CognitiveMemoryOps>,
}

impl SharedMem {
    fn register(root: &Path) -> Self {
        clear_in_process_writer();
        let writer: Arc<dyn CognitiveMemoryOps> =
            Arc::new(LibraryCognitiveMemory::open(root).expect("open shared cognitive memory"));
        register_in_process_writer(root.to_path_buf(), Arc::clone(&writer));
        Self { writer }
    }

    fn ops(&self) -> &dyn CognitiveMemoryOps {
        self.writer.as_ref()
    }
}

impl Drop for SharedMem {
    fn drop(&mut self) {
        clear_in_process_writer();
    }
}

/// A cognitive-memory backend that simulates a memory-ipc transport failure on
/// the read path, mirroring the real "bridge 'memory-ipc' transport error" the
/// #2896 bug reports. Non-faulted operations return benign values.
struct FaultyMemory {
    fail_reads: bool,
}

impl FaultyMemory {
    fn transport_err(op: &str) -> SimardError {
        SimardError::RpcCallFailed {
            bridge: "memory-ipc".to_string(),
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

/// A `TempDir` state root (never under `$HOME/.simard`) paired with a
/// fault backend registered as the in-process writer.
struct FaultFixture {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    _mem: Arc<dyn CognitiveMemoryOps>,
}

impl FaultFixture {
    fn register_failing_reads() -> Self {
        clear_in_process_writer();
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let mem: Arc<dyn CognitiveMemoryOps> = Arc::new(FaultyMemory { fail_reads: true });
        register_in_process_writer(root.clone(), Arc::clone(&mem));
        Self {
            _dir: dir,
            root,
            _mem: mem,
        }
    }
}

/// Build a Proposed `goal-store:record` exactly as the creative-ideas router
/// (`route_idea_to_goal`) does when an operator promotes an idea.
fn proposed_record(title: &str) -> GoalRecord {
    GoalRecord {
        slug: goal_slug(title),
        title: title.to_string(),
        rationale: "routed from a promoted creative idea".to_string(),
        status: GoalStatus::Proposed,
        priority: 3,
        owner_identity: "creative-ideas".to_string(),
        source_session_id: SessionId::parse("session-018f1f7e-4c5d-7b2a-8f10-b5c0d4f7b123")
            .expect("session id should parse"),
        updated_in: SessionPhase::Planning,
        evidence: Vec::new(),
        labels: vec![crate::goal_curation::labels::SOURCE_CREATIVE_IDEAS.to_string()],
    }
}

// ---------------------------------------------------------------------------
// dashboard_live_goal_board — the live union builder
// ---------------------------------------------------------------------------

/// The #2922 core: a Proposed record persisted to the live goal store must show
/// up in the live board WITHOUT any snapshot write in between.
#[test]
#[serial_test::serial(cognitive_memory)]
fn live_builder_surfaces_proposed_goal_store_record_immediately() {
    let state = HermeticState::new();
    let root = state.state_root().to_path_buf();
    let mem = SharedMem::register(&root);
    // Base snapshot is EMPTY — the record must arrive purely via the live overlay.
    save_goal_board(&GoalBoard::new(), mem.ops()).expect("seed empty snapshot base");

    let title = "Ship the live goal-board read for issue 2922";
    let store = CognitiveMemoryGoalStore::new(root.clone()).expect("construct goal store");
    store
        .put(proposed_record(title))
        .expect("persist Proposed goal record via the live store");

    // No snapshot write happens here — this is the whole point of the fix.
    let board = dashboard_live_goal_board(&root).expect("live board build must succeed");

    let slug = goal_slug(title);
    assert!(
        board
            .backlog
            .iter()
            .any(|b| b.id == slug || b.description == title),
        "a freshly-persisted Proposed goal-store record must appear in the live board \
         backlog without a snapshot cycle (issue #2922); got backlog {:?}",
        board.backlog
    );
    // It is Proposed, so it belongs in backlog, not active.
    assert!(
        !board.active.iter().any(|g| g.id == slug),
        "a Proposed record must not land in the active list"
    );
}

/// Dedup contract: a goal-store record whose slug already exists on the
/// authoritative snapshot base is dropped — the base wins and carries the richer
/// fields.
#[test]
#[serial_test::serial(cognitive_memory)]
fn live_builder_dedups_by_slug_base_wins() {
    let state = HermeticState::new();
    let root = state.state_root().to_path_buf();
    let mem = SharedMem::register(&root);

    let mut base = GoalBoard::new();
    base.active.push(ActiveGoal {
        labels: Vec::new(),
        parent_goal_id: None,
        priority_explicit: false,
        repo: Some("Simard".to_string()),
        id: "shared-slug-goal".to_string(),
        description: "BASE authoritative description".to_string(),
        priority: 1,
        status: GoalProgress::InProgress { percent: 70 },
        assigned_to: Some("simard".to_string()),
        current_activity: Some("advancing on the base goal".to_string()),
        wip_refs: vec![],
        last_progress_update_at: None,
    });
    save_goal_board(&base, mem.ops()).expect("seed base snapshot with the active goal");

    // Overlay record colliding on the SAME slug, with losing content.
    let store = CognitiveMemoryGoalStore::new(root.clone()).expect("construct goal store");
    let mut rec = proposed_record("overlay title that must lose the dedup");
    rec.slug = "shared-slug-goal".to_string();
    store.put(rec).expect("persist colliding overlay record");

    let board = dashboard_live_goal_board(&root).expect("live board build");

    let matches: Vec<&ActiveGoal> = board
        .active
        .iter()
        .filter(|g| g.id == "shared-slug-goal")
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "a slug collision must not double-render the goal; got active {:?}",
        board.active
    );
    assert_eq!(
        matches[0].description, "BASE authoritative description",
        "the authoritative snapshot goal must win on a slug collision, keeping its rich fields"
    );
    assert!(
        !board.backlog.iter().any(|b| b.id == "shared-slug-goal"),
        "the colliding overlay record must be dropped, never added as a duplicate backlog item"
    );
}

/// A Completed overlay record is terminal and must not be surfaced on the live
/// board in either bucket.
#[test]
#[serial_test::serial(cognitive_memory)]
fn live_builder_skips_completed_overlay_records() {
    let state = HermeticState::new();
    let root = state.state_root().to_path_buf();
    let mem = SharedMem::register(&root);
    save_goal_board(&GoalBoard::new(), mem.ops()).expect("seed empty snapshot base");

    let title = "A completed overlay goal that must not surface";
    let store = CognitiveMemoryGoalStore::new(root.clone()).expect("construct goal store");
    let mut rec = proposed_record(title);
    rec.status = GoalStatus::Completed;
    store.put(rec).expect("persist completed overlay record");

    let board = dashboard_live_goal_board(&root).expect("live board build");

    let slug = goal_slug(title);
    assert!(
        !board.active.iter().any(|g| g.id == slug),
        "a Completed overlay record must not appear in active"
    );
    assert!(
        !board.backlog.iter().any(|b| b.id == slug),
        "a Completed overlay record must not appear in backlog"
    );
}

/// Fail-closed: a transport fault on the live read must PROPAGATE as `Err`,
/// never a masked empty board (that would reintroduce the exact silent staleness
/// #2922 removes).
#[test]
#[serial_test::serial(cognitive_memory)]
fn live_builder_surfaces_reader_transport_error() {
    let fx = FaultFixture::register_failing_reads();

    let result = dashboard_live_goal_board(&fx.root);

    clear_in_process_writer();
    assert!(
        result.is_err(),
        "a broken-pipe read on either leg (snapshot base or goal-store overlay) MUST \
         propagate as Err, never a phantom empty board (issue #2922 fail-closed). Got: {result:?}",
    );
}

// ---------------------------------------------------------------------------
// GET /api/goals — goals_at repointed at the live builder
// ---------------------------------------------------------------------------

/// End-to-end acceptance: promoting a creative idea persists a Proposed record;
/// `goals_at` must surface it in `backlog` and increment `backlog_count` on the
/// very next poll, with NO snapshot write.
#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn goals_at_shows_proposed_record_without_snapshot_write() {
    let state = HermeticState::new();
    let root = state.state_root().to_path_buf();
    let mem = SharedMem::register(&root);
    save_goal_board(&GoalBoard::new(), mem.ops()).expect("seed empty snapshot base");

    let title = "Promoted creative-idea goal (2922 acceptance)";
    let store = CognitiveMemoryGoalStore::new(root.clone()).expect("construct goal store");
    store
        .put(proposed_record(title))
        .expect("persist Proposed record via the live store");

    // Deliberately NO snapshot write between the put and the read.
    let result = goals_at(&root).await;
    let val = &result.0;

    let backlog = val["backlog"].as_array().expect("backlog must be an array");
    assert!(
        backlog.iter().any(|b| b["description"] == title),
        "goals_at must surface the freshly-promoted Proposed goal in backlog WITHOUT a \
         snapshot cycle (issue #2922); got {val}"
    );
    assert!(
        val["backlog_count"].as_u64().unwrap_or(0) >= 1,
        "backlog_count must reflect the live union; got {val}"
    );
    // Shape preserved and successful → no error field.
    assert!(
        val.get("error").map(|e| e.is_null()).unwrap_or(true),
        "a successful live read must not carry an error field; got {val}"
    );
    drop(mem);
}

/// Fail-closed handler contract: on a live-read failure `goals_at` returns an
/// explicit error payload with zeroed counts and empty lists — never a
/// silently-empty board that would be indistinguishable from "no goals".
#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn goals_at_fail_closed_surfaces_error_not_stale_board() {
    let fx = FaultFixture::register_failing_reads();

    let result = goals_at(&fx.root).await;

    clear_in_process_writer();
    let val = &result.0;
    assert!(
        val.get("error").and_then(|e| e.as_str()).is_some(),
        "a live-read failure MUST surface an explicit error field, never a silently-empty \
         board (issue #2922 fail-closed); got {val}"
    );
    assert_eq!(
        val["active_count"].as_u64().unwrap_or(u64::MAX),
        0,
        "fail-closed payload must zero active_count; got {val}"
    );
    assert_eq!(
        val["backlog_count"].as_u64().unwrap_or(u64::MAX),
        0,
        "fail-closed payload must zero backlog_count; got {val}"
    );
    assert!(
        val["active"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false),
        "fail-closed payload must carry an empty active array; got {val}"
    );
    assert!(
        val["backlog"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false),
        "fail-closed payload must carry an empty backlog array; got {val}"
    );
}

// ---------------------------------------------------------------------------
// GET /api/memory — goal_records tile from the live union
// ---------------------------------------------------------------------------

/// The Goal-Records tile count must come from the live union and its `source`
/// label must be relabeled off the stale snapshot.
#[tokio::test]
#[serial_test::serial(cognitive_memory)]
async fn memory_metrics_goal_count_reflects_live_union_and_relabels_source() {
    let state = HermeticState::new();
    let root = state.state_root().to_path_buf();
    let mem = SharedMem::register(&root);
    save_goal_board(&GoalBoard::new(), mem.ops()).expect("seed empty snapshot base");

    let store = CognitiveMemoryGoalStore::new(root.clone()).expect("construct goal store");
    store
        .put(proposed_record("Live-counted proposal (2922)"))
        .expect("persist Proposed record via the live store");

    let result = memory_metrics().await;
    let gr = &result.0["goal_records"];

    assert_eq!(
        gr["source"], "cognitive-memory:live-goal-board",
        "goal_records.source must be relabeled off the snapshot to reflect the live read; got {gr}"
    );
    assert!(
        gr["count"].as_u64().unwrap_or(0) >= 1,
        "goal_records.count must include the live goal-store record without a snapshot cycle; got {gr}"
    );
    drop(mem);
}

// ---------------------------------------------------------------------------
// Outside-in end-to-end (Step 13): drive the REAL dashboard router over raw
// HTTP/1.1 on an ephemeral loopback port, exactly as the browser Goals tab
// does. Unlike the handler-fn tests above, this exercises the FULL consumer
// path — route registration, the `require_auth` layer, `resolve_state_root`,
// the live in-process goal store, and JSON serialization — proving that a
// freshly-persisted Proposed goal is visible over HTTP WITHOUT a snapshot
// cycle (issue #2922). Auth uses the deterministic `SIMARD_DASHBOARD_TOKEN`
// bearer, independent of the process `LOGIN_CODE`.
// ---------------------------------------------------------------------------

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ITEST_TOKEN: &str = "itest-live-goal-board";

/// One-shot HTTP/1.1 request over a raw socket → `(status_code, body)`.
/// `Connection: close` lets the server delimit the body by EOF so
/// `read_to_end` completes with no HTTP-client dependency.
async fn http_request(addr: SocketAddr, path: &str, bearer: Option<&str>) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect to ephemeral dashboard server");
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if let Some(b) = bearer {
        req.push_str(&format!("Authorization: Bearer {b}\r\n"));
    }
    req.push_str("\r\n");
    stream
        .write_all(req.as_bytes())
        .await
        .expect("write request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read response");
    let text = String::from_utf8_lossy(&raw).into_owned();
    let code = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (code, body)
}

/// [`http_request`] wrapped in a 30s timeout so a wiring bug can never hang the
/// suite.
async fn http(addr: SocketAddr, path: &str, bearer: Option<&str>) -> (u16, String) {
    tokio::time::timeout(Duration::from_secs(30), http_request(addr, path, bearer))
        .await
        .unwrap_or_else(|_| panic!("GET {path} timed out"))
}

/// Boot the real [`build_router`](super::routes::build_router) on an ephemeral
/// loopback port (auth initialized) and return its address.
async fn spawn_dashboard() -> SocketAddr {
    super::auth::init_login_code();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind ephemeral loopback port");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, super::routes::build_router()).await;
    });
    addr
}

// SAFETY (both fns): env mutation is serialised by
// `#[serial_test::serial(cognitive_memory)]`; the token is set before any
// request that reads it and cleared only after all responses are received.
fn set_dashboard_token() {
    unsafe { std::env::set_var("SIMARD_DASHBOARD_TOKEN", ITEST_TOKEN) };
}
fn clear_dashboard_token() {
    unsafe { std::env::remove_var("SIMARD_DASHBOARD_TOKEN") };
}

/// Outside-in acceptance (#2922): over the REAL router, a promoted creative-idea
/// Proposed goal persisted to the LIVE store surfaces on `GET /api/goals`
/// (backlog + count) and is counted by the `GET /api/memory` goal-records tile
/// (relabeled off the stale snapshot) — all WITHOUT any snapshot write, and
/// gated behind auth (unauthenticated ⇒ 401, so goal data never leaks).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial(cognitive_memory)]
async fn http_goals_board_surfaces_live_proposed_goal_without_snapshot() {
    let state = HermeticState::new();
    let root = state.state_root().to_path_buf();
    let mem = SharedMem::register(&root);
    // Base snapshot is EMPTY — the goal must arrive purely via the live overlay.
    save_goal_board(&GoalBoard::new(), mem.ops()).expect("seed empty snapshot base");

    let title = "Ship the live goal-board read over HTTP (2922 e2e)";
    let store = CognitiveMemoryGoalStore::new(root.clone()).expect("construct goal store");
    store
        .put(proposed_record(title))
        .expect("persist Proposed goal record via the live store");

    let addr = spawn_dashboard().await;

    // Auth-gated: no bearer ⇒ 401, so the board never leaks past the auth layer.
    let (unauth, _) = http(addr, "/api/goals", None).await;
    assert_eq!(unauth, 401, "the goal-board endpoint must sit behind auth");

    set_dashboard_token();
    // Deliberately NO snapshot write between the put and the HTTP read.
    let (code, body) = http(addr, "/api/goals", Some(ITEST_TOKEN)).await;
    let (mem_code, mem_body) = http(addr, "/api/memory", Some(ITEST_TOKEN)).await;
    clear_dashboard_token();

    assert_eq!(
        code, 200,
        "authenticated goal-board load must succeed; body={body:?}"
    );
    let v: serde_json::Value =
        serde_json::from_str(&body).expect("/api/goals returns a JSON object");
    let backlog = v["backlog"].as_array().expect("backlog must be an array");
    assert!(
        backlog.iter().any(|b| b["description"] == title),
        "GET /api/goals must surface the freshly-promoted Proposed goal in backlog WITHOUT a \
         snapshot cycle (issue #2922); got {v}"
    );
    assert!(
        v["backlog_count"].as_u64().unwrap_or(0) >= 1,
        "backlog_count must reflect the live union over HTTP; got {v}"
    );
    assert!(
        v.get("error").map(|e| e.is_null()).unwrap_or(true),
        "a successful live read must not carry an error field; got {v}"
    );

    // The Memory tab's Goal-Records tile counts the same live union, relabeled.
    assert_eq!(
        mem_code, 200,
        "authenticated memory load must succeed; body={mem_body:?}"
    );
    let mv: serde_json::Value =
        serde_json::from_str(&mem_body).expect("/api/memory returns a JSON object");
    let gr = &mv["goal_records"];
    assert_eq!(
        gr["source"], "cognitive-memory:live-goal-board",
        "goal_records.source must be relabeled off the snapshot over HTTP; got {gr}"
    );
    assert!(
        gr["count"].as_u64().unwrap_or(0) >= 1,
        "goal_records.count must include the live goal-store record over HTTP; got {gr}"
    );
    drop(mem);
}
