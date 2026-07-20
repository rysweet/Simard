//! The OS-signalling seam used by [`super::GroupChild`] to tear down a whole
//! process group.
//!
//! Cross-links `rysweet/amplihack-rs#964` (same bug class: a failed/aborted
//! orchestrator run leaking recursively-spawned subprocesses). Abstracting the
//! two raw `libc::kill` operations behind a trait lets the teardown logic be
//! unit-tested **offline, serially, and sleep-free**: production wires in
//! [`LibcSignaller`]; tests inject [`RecordingProbe`], which records every
//! signal instead of touching a real process group.
//!
//! # Safety invariants (REQ-V1/REQ-V2)
//!
//! Every implementation MUST refuse a non-positive `pgid` (`0` = the caller's
//! own group, `1` = init / broadcast target, negative = nonsensical) *before*
//! issuing any negative-target signal, so a stray teardown can never reach the
//! caller or every process on the host (fail-closed). Signalling is
//! numeric-PID `libc::kill(-pgid, …)` only — never `pkill`/`killall`/name-based
//! kills (repo shell policy).

use std::io;

/// Abstraction over process-group signalling.
///
/// A [`super::GroupChild`] never calls `libc::kill` directly; it drives the two
/// operations below through this trait so the teardown escalation
/// (SIGTERM → bounded grace → SIGKILL) can be asserted deterministically in
/// tests without spawning real processes.
pub trait ProcessGroupProbe: Send + Sync {
    /// Send `signal` to the whole process group led by `pgid`
    /// (semantically `kill(-pgid, signal)`).
    ///
    /// Implementations MUST reject `pgid <= 1` with an error and issue **no**
    /// signal in that case (REQ-V1, fail-closed): `-0` targets the caller's own
    /// group and `-1` broadcasts to every process. Real child PIDs are always
    /// `> 1`.
    fn signal_group(&self, pgid: i32, signal: i32) -> io::Result<()>;

    /// Whether the process group led by `pgid` still has at least one live
    /// member (semantically `kill(-pgid, 0) == Ok`). Used to decide whether a
    /// graceful SIGTERM must be escalated to SIGKILL.
    ///
    /// MUST return `false` for `pgid <= 1` without issuing any signal.
    fn group_alive(&self, pgid: i32) -> bool;
}

/// Production signaller: numeric-PID `libc::kill(-pgid, …)` only.
///
/// The `pgid > 1` invariant is enforced here as well as in `GroupChild`
/// (defence in depth) so no code path can ever hand a `0`/`1`/negative target
/// to the FFI `kill`. Mirrors the repo's existing shell-free signal policy in
/// `meeting_backend::agent_proxy::kill_process_group` and
/// `self_deploy::orphan::send_signal`.
pub struct LibcSignaller;

impl ProcessGroupProbe for LibcSignaller {
    fn signal_group(&self, pgid: i32, signal: i32) -> io::Result<()> {
        // REQ-V1 (fail-closed): `-0` targets the caller's own group and `-1`
        // broadcasts to every process on the host. Refuse both — and any
        // negative pgid — BEFORE issuing any negative-target `kill`. Real child
        // PIDs are always > 1.
        if pgid <= 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("refusing to signal non-positive pgid {pgid} (REQ-V1)"),
            ));
        }
        // SAFETY: `libc::kill` is FFI but well-defined for any `(pid, signal)`
        // pair. The negated group-leader PID `-pgid` targets exactly the child's
        // own process group (created via `process_group(0)`); it cannot reach
        // this process. The `pgid > 1` guard above rules out the caller's group
        // and the broadcast target.
        let rc = unsafe { libc::kill(-pgid, signal) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn group_alive(&self, pgid: i32) -> bool {
        // Never probe a non-positive pgid: `kill(-0/-1, 0)` would inspect the
        // caller's own group or broadcast. Fail-closed to "not alive".
        if pgid <= 1 {
            return false;
        }
        // SAFETY: signal `0` performs existence/permission checking without
        // delivering a signal. `rc == 0` means at least one group member exists
        // and is signallable; `ESRCH` (group gone) yields a non-zero return.
        let rc = unsafe { libc::kill(-pgid, 0) };
        rc == 0
    }
}

/// Test double: records every `signal_group` call in order and answers
/// `group_alive` from a scripted flag, so teardown escalation is fully
/// deterministic without a real process group.
#[cfg(test)]
#[derive(Default)]
pub struct RecordingProbe {
    /// `(pgid, signal)` for each `signal_group` call, in call order.
    signals: std::sync::Mutex<Vec<(i32, i32)>>,
    /// Scripted answer for `group_alive` (models a child that ignores SIGTERM
    /// and therefore requires SIGKILL escalation when `true`).
    alive: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl RecordingProbe {
    pub fn new() -> Self {
        Self::default()
    }

    /// Script whether the group appears alive when `group_alive` is polled.
    pub fn set_alive(&self, alive: bool) {
        self.alive.store(alive, std::sync::atomic::Ordering::SeqCst);
    }

    /// The ordered list of `(pgid, signal)` pairs signalled so far.
    pub fn recorded(&self) -> Vec<(i32, i32)> {
        self.signals.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl ProcessGroupProbe for RecordingProbe {
    fn signal_group(&self, pgid: i32, signal: i32) -> io::Result<()> {
        self.signals.lock().unwrap().push((pgid, signal));
        Ok(())
    }

    fn group_alive(&self, _pgid: i32) -> bool {
        self.alive.load(std::sync::atomic::Ordering::SeqCst)
    }
}
