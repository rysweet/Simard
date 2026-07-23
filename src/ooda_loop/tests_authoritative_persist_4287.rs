//! TDD regression tests for issue #4287 — "run_ooda_cycle bypasses the
//! authoritative goal-board store for direct callers".
//!
//! # The contract these tests pin
//!
//! The daemon loop persists each completed cycle through the authoritative,
//! lock-serialised file store ([`crate::goal_board_store::commit_cycle`], which
//! writes `<state_root>/state/goal_board.json`). A **direct** caller of
//! [`crate::ooda_loop::run_ooda_cycle`] (e.g. the cognitive-thread OODA tick, or
//! any embedder) must reach the SAME authoritative store — otherwise the
//! in-memory board it just curated is written only to the cognitive-memory
//! snapshot and the durable file diverges, so a subsequent process (or the
//! daemon on its next restart) reads a stale board.
//!
//! Today `run_ooda_cycle`'s persist block calls
//! [`crate::goal_curation::save_goal_board_with_removals`] /
//! [`crate::goal_curation::persist_board`] (cognitive memory only) and never
//! touches `goal_board.json`. These tests therefore **fail against the current
//! code** (the authoritative file is absent / stale after a cycle) and pass once
//! the persist block is routed through `goal_board_store::commit_cycle`.
//!
//! Every test is hermetic: [`HermeticState`] pins `SIMARD_STATE_ROOT` to a fresh
//! `TempDir` and the clients are in-memory RPC mocks, so no real `~/.simard`, no
//! network, and no live LLM is touched. They are keyed into the
//! `serial(cognitive_memory)` group because `HermeticState` mutates
//! process-global env.

use std::sync::Arc;

use serde_json::json;

use crate::goal_curation::{ActiveGoal, GoalBoard};
use crate::gym_client::GymClient;
use crate::knowledge_client::KnowledgeClient;
use crate::ooda_loop::{OodaClients, OodaConfig, OodaState, connect_memory, run_ooda_cycle};
use crate::rpc::RpcErrorPayload;
use crate::rpc_transport::InMemoryRpcTransport;
use crate::test_support::HermeticState;

fn mock_knowledge() -> KnowledgeClient {
    KnowledgeClient::new(Box::new(InMemoryRpcTransport::new(
        "test-4287-knowledge",
        |method, _params| match method {
            "knowledge.list_packs" => Ok(json!({ "packs": [] })),
            other => Err(RpcErrorPayload {
                code: -32601,
                message: format!("unknown method: {other}"),
            }),
        },
    )))
}

fn mock_gym() -> GymClient {
    GymClient::new(Box::new(InMemoryRpcTransport::new(
        "test-4287-gym",
        |_method, _params| Ok(json!({ "suite_id": "test", "success": true })),
    )))
}

/// Build a minimal `OodaClients` whose cognitive memory is a REAL in-process
/// [`crate::cognitive_memory::LibraryCognitiveMemory`] opened on the hermetic
/// `state_root` (via [`connect_memory`]) so a full cycle's memory RPCs all
/// resolve. Knowledge and gym stay as in-memory stubs — the cycle treats their
/// unavailability as best-effort and continues.
fn test_clients(state_root: &std::path::Path) -> OodaClients {
    OodaClients {
        memory: connect_memory(state_root).expect("open in-process cognitive memory"),
        knowledge: mock_knowledge(),
        gym: mock_gym(),
        session: None,
        session_factory: None,
        brain: Arc::new(crate::ooda_brain::DeterministicLifecycleBrain),
        decide_brain: None,
        orient_brain: None,
        repo_root: std::path::PathBuf::from("."),
        progress_evidence: Arc::new(
            crate::goal_curation::progress_evidence::NoopProgressEvidenceChecker,
        ),
        completion_evidence: None,
        outcome_verify_brain: None,
        live_signals: None,
    }
}

/// After a direct `run_ooda_cycle`, the authoritative on-disk store
/// (`<state_root>/state/goal_board.json`) must reflect the cycle's board.
///
/// RED today: the direct persist path writes only the cognitive-memory
/// snapshot, so `goal_board_store::load(state_root)` returns an EMPTY board and
/// the active goal the cycle carried is absent. GREEN once the persist block is
/// routed through `goal_board_store::commit_cycle`.
#[test]
#[serial_test::serial(cognitive_memory)]
fn direct_run_ooda_cycle_persists_board_to_authoritative_store() {
    let hermetic = HermeticState::new();
    let state_root = hermetic.state_root();

    // Seed the in-memory board with one plain, NotStarted goal. With no
    // completion evidence and a single cycle it cannot archive or trip the
    // no-progress breaker, so it must survive the cycle and be persisted.
    let mut board = GoalBoard::new();
    board.active.push(ActiveGoal::new(
        "persist-4287",
        "goal that must reach disk",
        90,
    ));
    let mut state = OodaState::new(board);
    let mut clients = test_clients(state_root);
    let config = OodaConfig::default();

    run_ooda_cycle(&mut state, &mut clients, &config).expect("cycle should complete");

    // The authoritative file store — the SAME one the daemon and the `simard
    // goal` CLI read — must now contain the goal.
    let persisted = crate::goal_board_store::load(state_root);
    assert!(
        persisted
            .board
            .active
            .iter()
            .any(|g| g.id == "persist-4287"),
        "issue #4287: a direct run_ooda_cycle must persist through the \
         authoritative goal_board_store (goal_board.json), but the on-disk \
         store held {:?}",
        persisted
            .board
            .active
            .iter()
            .map(|g| g.id.as_str())
            .collect::<Vec<_>>()
    );
}

/// The authoritative store must be *read-your-writes* across an independent
/// reader: a fresh `goal_board_store::load` (modelling a separate process / the
/// daemon after a restart) sees exactly what the direct cycle curated.
///
/// RED today for the same reason as above — the file is never written by the
/// direct path, so an independent reader sees an empty board.
#[test]
#[serial_test::serial(cognitive_memory)]
fn authoritative_store_is_read_your_writes_for_independent_reader() {
    let hermetic = HermeticState::new();
    let state_root = hermetic.state_root();

    let mut board = GoalBoard::new();
    board
        .active
        .push(ActiveGoal::new("ryw-4287", "read-your-writes goal", 80));
    let mut state = OodaState::new(board);
    let mut clients = test_clients(state_root);
    let config = OodaConfig::default();

    run_ooda_cycle(&mut state, &mut clients, &config).expect("cycle should complete");

    // Model a second, independent caller opening the store cold.
    let independent = crate::goal_board_store::load(state_root);
    assert!(
        independent.board.active.iter().any(|g| g.id == "ryw-4287"),
        "issue #4287: an independent reader of the authoritative store must \
         observe the direct cycle's committed board (read-your-writes)"
    );
}
