//! Integration test for cognitive-memory durability across SIGTERM
//! (issue #1631). Spawns the `sigterm_durability_helper` example binary,
//! writes 10 facts, sends SIGTERM via `nix::sys::signal::kill` (no shell
//! out to /bin/kill), waits for clean exit, then reopens the DB and asserts
//! all 10 facts survive. Includes a negative-control SIGKILL run that
//! proves the test framework is sensitive enough to detect data loss.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use simard::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};

const N_FACTS: usize = 10;

fn helper_binary() -> PathBuf {
    // Cargo places example binaries next to integration test binaries
    // under target/<profile>/examples/<name>.
    let exe = std::env::current_exe().expect("current_exe");
    // exe is .../target/<profile>/deps/<test_binary>-<hash>
    let deps = exe.parent().expect("deps dir");
    let profile = deps.parent().expect("profile dir");
    let candidate = profile.join("examples").join("sigterm_durability_helper");
    if candidate.exists() {
        return candidate;
    }
    panic!(
        "sigterm_durability_helper binary not found at {}; \
         build with: cargo build --example sigterm_durability_helper --release",
        candidate.display()
    );
}

fn spawn_helper(state_root: &Path) -> Child {
    Command::new(helper_binary())
        .arg(state_root)
        .arg(N_FACTS.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn helper")
}

fn read_ready_pid(child: &mut Child) -> u32 {
    let stdout = child.stdout.take().expect("stdout pipe");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        line.clear();
        let n = reader.read_line(&mut line).expect("read READY");
        if n == 0 {
            panic!("helper exited before printing READY");
        }
        if let Some(rest) = line.trim().strip_prefix("READY ") {
            return rest.parse().expect("parse pid");
        }
        if Instant::now() > deadline {
            panic!("timed out waiting for READY: last line: {line:?}");
        }
    }
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> std::io::Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait()? {
            Some(_) => return Ok(true),
            None => {
                if Instant::now() > deadline {
                    return Ok(false);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn count_facts(state_root: &Path) -> usize {
    let mem = NativeCognitiveMemory::open(state_root).expect("reopen DB");
    let facts = mem
        .search_facts("durability-fact", 1000, 0.0)
        .expect("search_facts");
    facts.len()
}

#[test]
fn sigterm_preserves_all_writes_across_restart() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let state_root = tmp.path().join("simard-state");

    let mut child = spawn_helper(&state_root);
    let pid = read_ready_pid(&mut child);
    assert!(pid > 0, "child PID must be positive");

    // Send SIGTERM via libc/nix — never shell out to /bin/kill.
    kill(Pid::from_raw(pid as i32), Signal::SIGTERM).expect("send SIGTERM");

    let exited = wait_with_timeout(&mut child, Duration::from_secs(30)).expect("wait_with_timeout");
    if !exited {
        let _ = child.kill();
        panic!("helper did not exit within 30s of SIGTERM");
    }
    let status = child.wait().expect("wait");
    // The helper installs its own SIGTERM handler, so it exits 0.
    assert!(
        status.success(),
        "helper should exit 0 after SIGTERM, got {status:?}"
    );

    // Restart logic: simply re-open the DB and search for our facts.
    let count = count_facts(&state_root);
    assert_eq!(
        count, N_FACTS,
        "all {N_FACTS} facts must survive SIGTERM-driven shutdown (got {count})"
    );
}

/// SIGKILL durability test (promoted from negative-control by issue #1973).
///
/// Before issue #1973 this test only asserted `count <= N_FACTS` because
/// lbug stranded acknowledged writes in its WAL until `Database::drop` (which
/// SIGKILL bypasses), so any value in `0..=N_FACTS` was possible. The
/// per-write fsync barrier added by issue #1973 promotes every successful
/// mutating op to "on stable storage before return" — meaning **all**
/// `N_FACTS` writes that the helper completed before printing `READY` must
/// survive a subsequent SIGKILL.
///
/// This test paired with `sigterm_preserves_all_writes_across_restart` above
/// covers both shutdown shapes (clean SIGTERM, unannounced SIGKILL) with the
/// same `== N_FACTS` bar, which is the regression pin for the barrier.
#[test]
fn sigkill_preserves_all_acknowledged_writes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let state_root = tmp.path().join("simard-state");

    let mut child = spawn_helper(&state_root);
    let pid = read_ready_pid(&mut child);
    assert!(pid > 0);

    // All N_FACTS writes have already completed and returned Ok(()) by the
    // time the helper prints READY (see examples/sigterm_durability_helper.rs).
    // With the per-write barrier (issue #1973), each Ok(()) implies fsync-to-
    // stable-storage has happened, so SIGKILL here cannot lose any of them.
    kill(Pid::from_raw(pid as i32), Signal::SIGKILL).expect("send SIGKILL");

    let exited = wait_with_timeout(&mut child, Duration::from_secs(15)).expect("wait_with_timeout");
    if !exited {
        let _ = child.kill();
        panic!("helper did not exit within 15s of SIGKILL");
    }

    let count = count_facts(&state_root);
    assert_eq!(
        count, N_FACTS,
        "all {N_FACTS} writes that completed before READY must survive \
         SIGKILL — per-write fsync barrier (issue #1973) guarantees every \
         Ok(()) from a mutating op is durable before control returns. \
         Got {count} surviving facts."
    );
}
