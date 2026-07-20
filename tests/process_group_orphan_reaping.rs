//! End-to-end regression test for the nested-subprocess orphan guard
//! (`simard::process_group_guard`), the Simard-side hardening that cross-links
//! `rysweet/amplihack-rs#964`.
//!
//! The bug class: when a smart-orchestrator run FAILS / aborts / times out, the
//! recursively-spawned subprocess subtree is leaked — grandchildren reparent to
//! init and keep pipes and target directories open. This test proves the guard
//! closes that hole: an *armed* [`GroupChild`] dropped on a simulated failure
//! exit path tears down the **whole** process group, so a real grandchild
//! spawned by the child does NOT survive as an orphan.
//!
//! It is `#[cfg(unix)]` (process-group teardown is Unix-only) and drives the
//! real production [`LibcSignaller`] against a real subtree, unlike the offline
//! unit tests that assert the signalling *contract* via a recording double.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use simard::process_group_guard::{GroupChild, LibcSignaller};
use std::sync::Arc;

/// Best-effort liveness probe for a *specific* pid on Linux, mirroring
/// `self_deploy::orphan::process_alive`: no `/proc/<pid>` entry means the
/// process is gone, and a **zombie** (state `Z`) also counts as gone because it
/// has already released every file descriptor and cannot hold resources open.
/// A running orphan would appear here as a non-zombie `/proc` entry.
fn pid_running(pid: i32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => match stat.rfind(')') {
            // The state field is the first token after the last ')': `Z` = zombie.
            Some(close) => !stat[close + 1..].trim_start().starts_with('Z'),
            None => true,
        },
        Err(_) => false,
    }
}

/// Poll `pid` until it is no longer a running process, or `timeout` elapses.
fn wait_until_gone(pid: i32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !pid_running(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    !pid_running(pid)
}

/// Whether a `/proc/<pid>` entry exists at all — **including a zombie**. Unlike
/// [`pid_running`] (which treats a zombie as gone because it holds no
/// resources), this stays `true` for an un-`wait()`ed child lingering in state
/// `Z`. It is the probe that distinguishes "leader was reaped" (entry gone)
/// from "leader leaked as a zombie" (entry still present).
fn pid_entry_exists(pid: i32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// Poll until `pid` has no `/proc` entry at all (fully reaped), or `timeout`
/// elapses.
fn wait_until_reaped(pid: i32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !pid_entry_exists(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    !pid_entry_exists(pid)
}

/// Dropping an *armed* guard on a failure/abort exit path must reap the entire
/// process subtree, leaving no orphaned grandchild — the exact leak reported in
/// amplihack-rs#964, hardened here on the Simard side.
#[test]
fn armed_drop_reaps_the_whole_subtree_leaving_no_orphan() {
    // A shell that backgrounds a long-lived grandchild (`sleep`), prints its
    // pid, then blocks. The grandchild inherits the shell's process group, so a
    // single group teardown must reach it. Without the guard this `sleep` would
    // be orphaned to init when the guard's owner aborted.
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg("sleep 300 & echo $! ; wait")
        .stdout(Stdio::piped());

    // Real production signaller; short grace keeps the test bounded (`sleep`
    // does not trap SIGTERM, so graceful teardown suffices — no SIGKILL needed).
    let mut guard =
        GroupChild::spawn_with(&mut cmd, Arc::new(LibcSignaller), Duration::from_secs(2))
            .expect("spawn a real child subtree in its own process group");

    assert!(guard.pgid() > 1, "a real child pgid must be > 1");

    // Read the backgrounded grandchild's pid from the child's stdout.
    let stdout = guard
        .child_mut()
        .expect("armed guard owns its child")
        .stdout
        .take()
        .expect("child stdout was piped");
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .expect("read grandchild pid line");
    let grandchild_pid: i32 = line.trim().parse().expect("grandchild pid is an integer");

    assert!(
        pid_running(grandchild_pid),
        "grandchild {grandchild_pid} should be alive before teardown"
    );

    // Simulate the failure/abort exit path: the guard goes out of scope while
    // the subtree is still running.
    drop(guard);

    assert!(
        wait_until_gone(grandchild_pid, Duration::from_secs(10)),
        "armed drop must tear down the whole group; grandchild {grandchild_pid} was orphaned"
    );
}

/// Companion to the subtree test above: armed drop must also `wait()` the
/// **immediate leader child**, not just signal the group. `std::process::Child`
/// does not reap on its own `Drop`, so without an explicit `wait()` in the
/// guard the leader lingers as a zombie (state `Z`) — one leaked PID/handle per
/// armed teardown, precisely the exhaustion class this guard exists to prevent.
/// This asserts the leader's `/proc` entry is gone entirely (reaped), which a
/// zombie would keep alive.
#[test]
fn armed_drop_reaps_the_immediate_leader_child_no_zombie() {
    // Same shape as the subtree test: a shell leader that backgrounds a
    // grandchild and blocks, so the leader is alive at drop time and must be
    // both signalled and reaped.
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg("sleep 300 & echo $! ; wait")
        .stdout(Stdio::piped());

    let mut guard =
        GroupChild::spawn_with(&mut cmd, Arc::new(LibcSignaller), Duration::from_secs(2))
            .expect("spawn a real child subtree in its own process group");

    // The leader child's PID equals its pgid (spawned via `process_group(0)`).
    let leader_pid = guard.pgid();
    assert!(leader_pid > 1, "a real child pgid must be > 1");

    // Sync on the child actually running by reading the grandchild pid it prints.
    let stdout = guard
        .child_mut()
        .expect("armed guard owns its child")
        .stdout
        .take()
        .expect("child stdout was piped");
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .expect("read grandchild pid line");

    assert!(
        pid_entry_exists(leader_pid),
        "leader {leader_pid} should be alive before teardown"
    );

    // Simulate the failure/abort exit path.
    drop(guard);

    assert!(
        wait_until_reaped(leader_pid, Duration::from_secs(10)),
        "armed drop must wait() the leader child; leader {leader_pid} leaked as a zombie"
    );
}
