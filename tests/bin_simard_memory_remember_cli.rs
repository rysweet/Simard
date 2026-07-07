//! Process-boundary integration tests for the distiller's WRITE tools
//! `simard memory remember` / `simard memory remember-procedure` (issue #2679).
//!
//! These exercise the *external-service integration* that neither the
//! `operator_cli::memory` unit tests nor the in-crate `memory_ipc` gated-write
//! tests can: the **real `simard` binary**, spawned as a separate process,
//! reaching a **live memory IPC socket** and committing a fact through the
//! server-side write-boundary gate. This is the exact path the distiller agent
//! drives during a semantic-handoff pass — `SIMARD_MEMORY_SOCKET` is exported to
//! the agent, and every fact becomes one `simard memory remember` process.
//!
//! The whole point of #2679 is that the agent's writes ARE its output: there is
//! no `{ "facts": [...] }` document scraped back out of noisy stdout and
//! hand-deserialized. These tests pin that the binary honours the documented
//! exit-code contract (0 stored / 2 usage / 3 no daemon / 4 quarantined) against
//! a real gate, and that a grounded fact genuinely lands in semantic memory.

use std::sync::Arc;
use std::time::Duration;

use assert_cmd::Command;
use simard::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use simard::memory_ipc::{RemoteCognitiveMemory, spawn_server};
use tempfile::TempDir;

/// A live in-memory cognitive store fronted by a real IPC server on a unix
/// socket. Holds the `TempDir` and `ServerHandle` alive for the test's lifetime.
struct LiveDaemon {
    _dir: TempDir,
    sock: std::path::PathBuf,
    mem: Arc<dyn CognitiveMemoryOps>,
    _handle: simard::memory_ipc::ServerHandle,
}

impl LiveDaemon {
    /// Spawn a real server over a fresh in-memory backend and block until the
    /// socket is accepting connections.
    fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let sock = dir.path().join("memory.sock");
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
            sock,
            mem,
            _handle: handle,
        }
    }

    /// Seed a real episode so a fact citing its id is *grounded* by the gate's
    /// store-existence check. Returns the episode node id.
    fn seed_episode(&self, content: &str) -> String {
        self.mem
            .store_episode(content, "engineer-cycle", None)
            .expect("store_episode")
    }

    /// `simard memory remember ...` pinned to this daemon's socket, hermetic
    /// (no update check, no leaked state root). Mirrors how the subprocess
    /// distiller runner exports `SIMARD_MEMORY_SOCKET` to the agent.
    fn remember(&self, args: &[&str]) -> std::process::Output {
        let mut cmd = Command::cargo_bin("simard").expect("simard must build");
        cmd.env("SIMARD_NO_UPDATE_CHECK", "1")
            .env("SIMARD_MEMORY_SOCKET", &self.sock)
            .env_remove("SIMARD_STATE_ROOT")
            .arg("memory");
        cmd.args(args);
        cmd.output().expect("run simard memory remember")
    }

    /// A fresh client connection for read-back verification through the gate.
    fn client(&self) -> RemoteCognitiveMemory {
        RemoteCognitiveMemory::connect(&self.sock).expect("connect")
    }
}

fn code(out: &std::process::Output) -> i32 {
    out.status
        .code()
        .expect("process exited via signal, not code")
}

/// The headline external-service path: a grounded, well-formed fact written by
/// the *real binary* clears the gate (exit 0) and is genuinely retrievable from
/// semantic memory — no document ever parsed anywhere in the path.
#[test]
fn remember_cli_stores_grounded_fact_end_to_end() {
    let daemon = LiveDaemon::start();
    let episode_id = daemon.seed_episode("empty outcome list panicked the cycle");

    let out = daemon.remember(&[
        "remember",
        "--concept",
        "bug-pattern",
        "--content",
        "empty outcome list panics cycle",
        "--source-episode-id",
        &episode_id,
        "--pass-id",
        "pass-live-1",
    ]);

    assert_eq!(
        code(&out),
        0,
        "grounded fact must exit 0 (stored); stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("stored concept=bug-pattern"),
        "stdout must confirm the store; got {stdout:?}"
    );

    // Prove the write really reached semantic memory through the gate.
    let facts = daemon
        .client()
        .search_facts("bug-pattern", 10, 0.0)
        .expect("search_facts");
    assert!(
        facts
            .iter()
            .any(|f| f.content == "empty outcome list panics cycle"),
        "the committed fact must be present in semantic memory; got {facts:?}"
    );
}

/// A fact citing an episode id that does not exist is ungrounded: the server
/// gate quarantines it (exit 4) and nothing leaks into semantic memory. This is
/// the anti-hallucination guarantee, enforced at the process boundary.
#[test]
fn remember_cli_quarantines_ungrounded_fact() {
    let daemon = LiveDaemon::start();

    let out = daemon.remember(&[
        "remember",
        "--concept",
        "bug-pattern",
        "--content",
        "three or more words here",
        "--source-episode-id",
        "epi_does_not_exist",
        "--pass-id",
        "pass-live-2",
    ]);

    assert_eq!(
        code(&out),
        4,
        "ungrounded fact must exit 4 (quarantined); stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("quarantined"),
        "stderr must explain the quarantine"
    );

    let facts = daemon
        .client()
        .search_facts("bug-pattern", 10, 0.0)
        .expect("search_facts");
    assert!(
        facts.is_empty(),
        "no ungrounded fact may leak into semantic memory; got {facts:?}"
    );
}

/// With no reachable daemon there is deliberately NO un-gated on-disk fallback
/// (a direct open would bypass the authoritative gate), so the binary exits 3.
#[test]
fn remember_cli_no_daemon_exits_3() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dead_sock = dir.path().join("nonexistent.sock");

    let out = Command::cargo_bin("simard")
        .expect("simard must build")
        .env("SIMARD_NO_UPDATE_CHECK", "1")
        .env("SIMARD_MEMORY_SOCKET", &dead_sock)
        .env_remove("SIMARD_STATE_ROOT")
        .args([
            "memory",
            "remember",
            "--concept",
            "bug-pattern",
            "--content",
            "no daemon is listening here",
        ])
        .output()
        .expect("run simard memory remember");

    assert_eq!(
        code(&out),
        3,
        "missing daemon must exit 3 (no reachable daemon); stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no reachable memory daemon"),
        "stderr must name the missing daemon"
    );
}

/// A malformed invocation (required `--content` missing) is a usage error (exit
/// 2) and never reaches the daemon — parsing fails first.
#[test]
fn remember_cli_missing_required_flag_exits_2() {
    let out = Command::cargo_bin("simard")
        .expect("simard must build")
        .env("SIMARD_NO_UPDATE_CHECK", "1")
        .env_remove("SIMARD_STATE_ROOT")
        .args(["memory", "remember", "--concept", "bug-pattern"])
        .output()
        .expect("run simard memory remember");

    assert_eq!(
        code(&out),
        2,
        "missing --content must exit 2 (usage error); stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("missing required --content"),
        "stderr must name the missing flag"
    );
}

/// The companion procedure write tool commits through the same live socket and
/// returns a node id (exit 0) — the recurring-procedure half of the handoff.
#[test]
fn remember_procedure_cli_stores_end_to_end() {
    let daemon = LiveDaemon::start();
    let episode_id = daemon.seed_episode("fixed the bug by reading then editing then testing");

    let out = daemon.remember(&[
        "remember-procedure",
        "--name",
        "fix-and-verify",
        "--step",
        "read the failing file",
        "--step",
        "edit the offending code",
        "--step",
        "run cargo test",
        "--source-episode-id",
        &episode_id,
        "--pass-id",
        "pass-live-3",
    ]);

    assert_eq!(
        code(&out),
        0,
        "procedure write must exit 0 (stored); stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("stored name=fix-and-verify"),
        "stdout must confirm the procedure store"
    );
}
