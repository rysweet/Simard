//! Process-boundary integration test for the self-deploy **RpcHealth** canary
//! gate (`src/self_relaunch/gates.rs`).
//!
//! The gate's whole purpose is to refuse to swap in a candidate binary that
//! cannot reach the running memory daemon over RPC. The in-crate unit tests can
//! prove the gate *fails closed* (missing binary / timeout) and that its argv
//! *dispatches*, but only this test can prove the **positive** contract: a
//! genuinely healthy candidate, dialing a **real live daemon** over a real unix
//! socket, passes.
//!
//! Shape (mirrors `bin_simard_memory_remember_cli.rs`):
//!   * Bring up a real memory IPC server on a socket under an isolated
//!     `SIMARD_STATE_ROOT` (so `memory stats` resolves the same socket via
//!     `socket_path_for`).
//!   * Run the lightweight canary gates (smoke, gym-baseline, rpc-health) against
//!     the **real `simard` binary** through the public `verify_canary` seam.
//!   * Assert every gate — including `rpc-health` — passes.
//!
//! `UnitTest` is deliberately excluded from the gate set here: it shells out to
//! `cargo test`, which would recurse into this very suite. Smoke + gym-baseline
//! + rpc-health exercise the real dial path without that recursion.
//!
//! `#[ignore]`d so the default `cargo test` (and the UnitTest canary gate that
//! shells out to it) never spawns a daemon or the release binary; run explicitly
//! with `cargo test --test self_deploy_rpc_health_gate -- --ignored`.

use std::sync::Arc;
use std::time::Duration;

use simard::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use simard::memory_ipc::{socket_path_for, spawn_server};
use simard::self_relaunch::{RelaunchConfig, RelaunchGate, all_gates_passed, verify_canary};
use tempfile::TempDir;

/// A real memory IPC server listening on the socket `memory stats` will resolve
/// for `state_root`. Holds the backend + server handle alive for the test.
struct LiveDaemon {
    _dir: TempDir,
    state_root: std::path::PathBuf,
    _mem: Arc<dyn CognitiveMemoryOps>,
    _handle: simard::memory_ipc::ServerHandle,
}

impl LiveDaemon {
    fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_root = dir.path().to_path_buf();
        // The gate re-injects the allow-listed SIMARD_STATE_ROOT, so the
        // candidate's `memory stats` dials exactly this socket.
        let sock = socket_path_for(&state_root);
        let mem: Arc<dyn CognitiveMemoryOps> =
            Arc::new(LibraryCognitiveMemory::in_memory().expect("in-memory db"));
        let handle = spawn_server(sock.clone(), Arc::clone(&mem)).expect("spawn server");
        for _ in 0..100 {
            if sock.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(sock.exists(), "server socket never appeared at {sock:?}");
        Self {
            _dir: dir,
            state_root,
            _mem: mem,
            _handle: handle,
        }
    }
}

#[test]
#[ignore = "spawns a live memory daemon and runs the release binary; run with --ignored"]
fn healthy_candidate_passes_rpc_health_against_a_live_daemon() {
    let daemon = LiveDaemon::start();

    // SIMARD_STATE_ROOT is on the canary env allow-list; the scrubbed gate env
    // re-injects it live at spawn so the candidate resolves the daemon's socket.
    // Set it on this process so `scrub_gate_env` can read it back.
    // SAFETY: this test file is its own test binary and defines a single test,
    // so no sibling test races on the process environment.
    unsafe {
        std::env::set_var("SIMARD_STATE_ROOT", &daemon.state_root);
    }

    let binary = std::path::PathBuf::from(env!("CARGO_BIN_EXE_simard"));
    let config = RelaunchConfig {
        canary_env: simard::self_relaunch::canary_gate_env_allowlist(),
        ..RelaunchConfig::default()
    };

    // Exclude UnitTest (it shells out to `cargo test` and would recurse into
    // this suite). The remaining gates all exercise the real candidate binary.
    let gates = [
        RelaunchGate::Smoke,
        RelaunchGate::GymBaseline,
        RelaunchGate::RpcHealth,
    ];

    let results = verify_canary(&binary, &gates, &config).expect("verify_canary ran");

    unsafe {
        std::env::remove_var("SIMARD_STATE_ROOT");
    }

    for r in &results {
        assert!(
            r.passed,
            "gate {} must pass for a healthy candidate against a live daemon: {}",
            r.gate, r.detail
        );
    }
    assert!(
        all_gates_passed(&results),
        "all lightweight canary gates must pass for a healthy candidate"
    );

    let rpc = results
        .iter()
        .find(|r| r.gate == RelaunchGate::RpcHealth)
        .expect("rpc-health gate present in results");
    assert!(
        rpc.passed,
        "rpc-health must GREEN when the candidate can dial the live daemon: {}",
        rpc.detail
    );
}
