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

/// A probe that really delivers signals and really checks liveness (wrapping the
/// production [`LibcSignaller`]) while recording every `signal_group` call, so a
/// real subtree can be torn down *and* the exact escalation sequence asserted.
struct RecordingLibcProbe {
    inner: LibcSignaller,
    signals: std::sync::Mutex<Vec<i32>>,
}

impl RecordingLibcProbe {
    fn new() -> Self {
        Self {
            inner: LibcSignaller,
            signals: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn recorded_signals(&self) -> Vec<i32> {
        self.signals.lock().unwrap().clone()
    }
}

impl simard::process_group_guard::ProcessGroupProbe for RecordingLibcProbe {
    fn signal_group(&self, pgid: i32, signal: i32) -> std::io::Result<()> {
        self.signals.lock().unwrap().push(signal);
        self.inner.signal_group(pgid, signal)
    }

    fn group_alive(&self, pgid: i32) -> bool {
        self.inner.group_alive(pgid)
    }
}

/// Regression: a real subtree that exits on the graceful SIGTERM must be torn
/// down with **SIGTERM only** — no SIGKILL escalation and no full-grace stall.
///
/// The hazard: `std::process::Child` does not reap on drop, so the leader
/// lingers as a zombie that `kill(-pgid, 0)` still counts as a live group
/// member. If the guard checks group liveness *without* first reaping the exited
/// leader, every teardown — even one whose subtree died instantly on SIGTERM —
/// waits the entire grace window and then escalates to a redundant SIGKILL. A
/// generous 30s grace makes that failure mode unmistakable: with the bug this
/// records `[SIGTERM, SIGKILL]` (after a 30s stall); fixed, it records
/// `[SIGTERM]` and returns promptly once the leader is reaped in-loop.
#[test]
fn graceful_group_is_reaped_without_sigkill_escalation() {
    // `sleep` does not trap SIGTERM, so the whole group dies on the graceful
    // signal. A long grace guarantees any spurious escalation is not merely a
    // race we happened to win.
    let mut cmd = Command::new("sleep");
    cmd.arg("300");

    let probe = Arc::new(RecordingLibcProbe::new());
    let guard = GroupChild::spawn_with(&mut cmd, probe.clone(), Duration::from_secs(30))
        .expect("spawn a real child in its own process group");
    let leader_pid = guard.pgid();
    assert!(leader_pid > 1, "a real child pgid must be > 1");

    let start = Instant::now();
    drop(guard); // graceful teardown of a group that dies on SIGTERM
    let elapsed = start.elapsed();

    assert_eq!(
        probe.recorded_signals(),
        vec![libc::SIGTERM],
        "a group that exits on SIGTERM must NOT be escalated to SIGKILL"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "graceful teardown must not stall for the full grace window (took {elapsed:?})"
    );
    assert!(
        wait_until_reaped(leader_pid, Duration::from_secs(10)),
        "leader {leader_pid} must be reaped, not leaked as a zombie"
    );
}
