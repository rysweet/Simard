//! Per-write fsync barrier crash-recovery integration test (issue #1973).
//!
//! Spawns the `cognitive_memory_crash_helper` example binary as a child
//! process, waits for it to confirm a single `store_fact` has returned
//! `Ok(())` (line `WROTE` on stdout), SIGKILLs it, then opens the same
//! `state_root` from a **fresh parent process** and asserts the written
//! fact survived. This is direct observable proof that the per-write fsync
//! barrier (`checkpoint() → fsync(data file) → fsync(parent dir)`) inside
//! `NativeCognitiveMemory`'s mutating ops has made the data durable
//! **before** the call returns to the caller — i.e. an acknowledged write
//! survives an unannounced crash.
//!
//! Goal G1 (per-write barrier) + Goal G2 (crash recovery proof) of epic
//! #1972 (improve-cognitive-memory-persistence).
//!
//! Without the barrier, this test is expected to fail: lbug buffers writes
//! in its WAL and only flushes on `Database::drop`, which SIGKILL bypasses.
//!
//! ===========================================================================
//! EXTRACTABLE FOR #1974 — DO NOT INLINE-EDIT THESE HELPERS
//! ===========================================================================
//! The helper functions below (`helper_binary`, `spawn_helper`,
//! `read_ready_pid`, `wait_for_marker`, `sigkill`, `wait_with_timeout`,
//! `count_facts_with_concept`) are intentionally module-scoped and
//! tagged `#[allow(dead_code)]` so issue #1974 can move this block
//! verbatim into `tests/support/crash_recovery.rs` without rewrites.
//! The existing `tests/daemon_sigterm_durability.rs` has near-identical
//! inline helpers; #1974 will deduplicate by importing from the support
//! module. Keep all functions in this block; do not interleave test code.
//! ===========================================================================

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use simard::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};

// ============================================================================
// BEGIN EXTRACTABLE-FOR-#1974 BLOCK
// ============================================================================

/// Locate the `cognitive_memory_crash_helper` example binary that Cargo
/// places at `target/<profile>/examples/<name>`.
#[allow(dead_code)]
fn helper_binary() -> PathBuf {
    let exe = std::env::current_exe().expect("current_exe");
    // exe = .../target/<profile>/deps/<test_binary>-<hash>
    let deps = exe.parent().expect("deps dir");
    let profile = deps.parent().expect("profile dir");
    let candidate = profile
        .join("examples")
        .join("cognitive_memory_crash_helper");
    if candidate.exists() {
        return candidate;
    }
    // In coverage / CI environments the example binary may not be built.
    // Return the path anyway — callers skip the test when it is missing.
    candidate
}

/// Spawn the helper with `state_root` as argv[1]. Pipes stdout/stderr so
/// the test can read the `READY` and `WROTE` markers.
#[allow(dead_code)]
fn spawn_helper(state_root: &Path, concept: &str, payload: &str) -> Child {
    Command::new(helper_binary())
        .arg(state_root)
        .arg(concept)
        .arg(payload)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn helper")
}

/// Read lines from the child's stdout until `READY <pid>` is seen, return
/// the parsed PID. Panics on EOF or timeout. The returned `BufReader` is
/// **moved back into the caller** via the second return slot so subsequent
/// markers can be read from the same stream without dropping buffered data.
#[allow(dead_code)]
fn read_ready_pid(child: &mut Child) -> (u32, BufReader<std::process::ChildStdout>) {
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
            let pid: u32 = rest.parse().expect("parse pid");
            return (pid, reader);
        }
        if Instant::now() > deadline {
            panic!("timed out waiting for READY: last line: {line:?}");
        }
    }
}

/// Wait for a specific marker line (e.g. `WROTE`) on the child's stdout.
/// Returns when the marker arrives; panics on EOF or timeout.
#[allow(dead_code)]
fn wait_for_marker(
    reader: &mut BufReader<std::process::ChildStdout>,
    marker: &str,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).expect("read marker");
        if n == 0 {
            panic!("helper exited before printing {marker}");
        }
        if line.trim() == marker {
            return;
        }
        if Instant::now() > deadline {
            panic!("timed out waiting for {marker}: last line: {line:?}");
        }
    }
}

/// Send SIGKILL to the given PID via `nix::sys::signal::kill`. Never shells
/// out to `/bin/kill`.
#[allow(dead_code)]
fn sigkill(pid: u32) {
    kill(Pid::from_raw(pid as i32), Signal::SIGKILL).expect("send SIGKILL");
}

/// Poll-wait for the child to exit. Returns `Ok(true)` if exited within
/// `timeout`, `Ok(false)` on timeout.
#[allow(dead_code)]
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> std::io::Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait()? {
            Some(_) => return Ok(true),
            None => {
                if Instant::now() > deadline {
                    return Ok(false);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Reopen the DB from a fresh process context and count facts whose
/// `concept` matches `concept` (case-sensitive substring search via
/// `CognitiveMemoryOps::search_facts`).
#[allow(dead_code)]
fn count_facts_with_concept(state_root: &Path, concept: &str) -> usize {
    let mem = NativeCognitiveMemory::open(state_root).expect("reopen DB from fresh process");
    let facts = mem.search_facts(concept, 1000, 0.0).expect("search_facts");
    facts.into_iter().filter(|f| f.concept == concept).count()
}

// ============================================================================
// END EXTRACTABLE-FOR-#1974 BLOCK
// ============================================================================

/// Core test: write one fact in the child, SIGKILL after `WROTE` is
/// observed by the parent, then reopen and assert the fact is present.
///
/// This is the direct observable proof of the per-write fsync barrier
/// (issue #1973). It must pass for **every** mutating op once the barrier
/// is applied to all `CognitiveMemoryOps` writers.
#[test]
fn sigkill_preserves_last_acknowledged_write() {
    let helper = helper_binary();
    if !helper.exists() {
        eprintln!("SKIP: cognitive_memory_crash_helper not built (coverage CI)");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let state_root = tmp.path().join("simard-state");
    let concept = "crash-marker-single";
    let payload = "single-write-payload";

    let mut child = spawn_helper(&state_root, concept, payload);
    let (pid, mut reader) = read_ready_pid(&mut child);
    assert!(pid > 0, "child PID must be positive");

    // Wait for the helper to confirm `store_fact` returned `Ok(())`. With
    // the per-write barrier in place, by the time `WROTE` arrives the
    // data is on stable storage (`checkpoint() → fsync(data) →
    // fsync(parent dir)` all complete).
    wait_for_marker(&mut reader, "WROTE", Duration::from_secs(30));

    sigkill(pid);

    let exited = wait_with_timeout(&mut child, Duration::from_secs(15)).expect("wait_with_timeout");
    if !exited {
        let _ = child.kill();
        panic!("helper did not exit within 15s of SIGKILL");
    }
    let status = child.wait().expect("wait");
    // SIGKILL terminates without a clean exit: status.success() must be false.
    assert!(
        !status.success(),
        "helper must NOT exit cleanly after SIGKILL — that would mean it ran shutdown code, invalidating the crash-recovery premise. status={status:?}"
    );

    // Fresh-process reopen + assertion. This is the crash-recovery proof.
    let count = count_facts_with_concept(&state_root, concept);
    assert_eq!(
        count, 1,
        "the one acknowledged write must survive SIGKILL — \
         the per-write fsync barrier (issue #1973) guarantees that any \
         `store_fact` call returning Ok(()) is on stable storage before \
         control returns to the caller. Got {count} surviving facts."
    );
}

/// Repeat the same crash + reopen cycle multiple times on the *same*
/// `state_root` to prove durability composes: each cycle's write must
/// survive the next cycle's crash. This catches latent issues such as
/// barrier-applied-only-on-first-write or recovery-path losing prior
/// committed data.
#[test]
fn repeated_sigkill_cycles_accumulate_writes() {
    let helper = helper_binary();
    if !helper.exists() {
        eprintln!("SKIP: cognitive_memory_crash_helper not built (coverage CI)");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let state_root = tmp.path().join("simard-state");
    const CYCLES: usize = 3;

    for i in 0..CYCLES {
        let concept = format!("crash-marker-cycle-{i}");
        let payload = format!("payload-cycle-{i}");

        let mut child = spawn_helper(&state_root, &concept, &payload);
        let (pid, mut reader) = read_ready_pid(&mut child);
        assert!(pid > 0);

        wait_for_marker(&mut reader, "WROTE", Duration::from_secs(30));
        sigkill(pid);

        let exited =
            wait_with_timeout(&mut child, Duration::from_secs(15)).expect("wait_with_timeout");
        if !exited {
            let _ = child.kill();
            panic!("cycle {i}: helper did not exit within 15s of SIGKILL");
        }

        // After each crash, every prior write (this one + all earlier) must
        // still be readable from a fresh process.
        for j in 0..=i {
            let prior_concept = format!("crash-marker-cycle-{j}");
            let count = count_facts_with_concept(&state_root, &prior_concept);
            assert_eq!(
                count, 1,
                "cycle {i}: write from cycle {j} ({prior_concept}) was lost — \
                 per-write fsync barrier must preserve all acknowledged writes \
                 across repeated unannounced crashes"
            );
        }
    }
}
