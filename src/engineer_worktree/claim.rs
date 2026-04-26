//! Per-engineer worktree claim helpers — PID + starttime sentinel.

use std::fs;
use std::path::Path;
use super::ENGINEER_CLAIM_FILE;

/// Read field 22 (starttime in jiffies since boot) from `/proc/<pid>/stat`.
/// Returns `None` if the file can't be read or is malformed.
///
/// `/proc/<pid>/stat` format: `pid (comm) state ppid ...` where `comm` may
/// itself contain spaces and parentheses. We must therefore find the LAST
/// `)` and split the remainder by whitespace; field 22 (starttime) is the
/// 20th token AFTER `comm` (state=1, ppid=2, ..., starttime=20).
#[cfg(unix)]
pub fn read_pid_starttime(pid: i32) -> Option<u64> {
    if pid <= 0 {
        return None;
    }
    let raw = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close_paren = raw.rfind(')')?;
    let after = raw.get(close_paren + 1..)?.trim_start();
    // After `comm`: state(1) ppid(2) pgrp(3) session(4) tty_nr(5) tpgid(6)
    // flags(7) minflt(8) cminflt(9) majflt(10) cmajflt(11) utime(12) stime(13)
    // cutime(14) cstime(15) priority(16) nice(17) num_threads(18)
    // itrealvalue(19) starttime(20)
    let mut tokens = after.split_ascii_whitespace();
    let starttime = tokens.nth(19)?;
    starttime.parse().ok()
}

#[cfg(not(unix))]
pub fn read_pid_starttime(_pid: i32) -> Option<u64> {
    None
}

/// Format the contents written into the engineer-claim sentinel file.
pub fn format_engineer_claim(pid: u32) -> String {
    match read_pid_starttime(pid as i32) {
        Some(st) => format!("{pid}\n{st}\n"),
        // /proc unavailable (test sandboxes, non-Linux): fall back to PID-only.
        // Read path treats absent starttime as "unverifiable", but kill(pid,0)
        // alone is still better than no claim.
        None => format!("{pid}\n"),
    }
}

/// Parsed engineer-claim sentinel contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineerClaim {
    pub pid: i32,
    /// Starttime in /proc jiffies, if recorded (None for pre-#1238 sentinels
    /// or platforms without /proc).
    pub starttime: Option<u64>,
}

/// Probe whether `pid` refers to a running process via `kill(pid, 0)`. Returns
/// `true` if the process exists (regardless of permission to signal it).
/// Returns `false` if the process is dead (ESRCH) or `pid` is non-positive.
#[cfg(unix)]
pub fn is_pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: kill(pid, 0) performs no signal delivery. It is the standard
    // POSIX liveness probe and has no side effects on the target process.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    // EPERM means the process exists but we can't signal it — still alive.
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    errno == libc::EPERM
}

#[cfg(not(unix))]
pub fn is_pid_alive(_pid: i32) -> bool {
    // Non-Unix platforms don't run the daemon; conservative default.
    true
}

/// Public wrapper over the private liveness probe so other modules
/// (e.g. `ooda_actions::advance_goal::find_live_engineer_for_goal`)
/// can implement their own claim-based checks without duplicating the
/// `kill(pid, 0)` logic.
pub fn is_pid_alive_public(pid: i32) -> bool {
    is_pid_alive(pid)
}

/// Public wrapper over `/proc/<pid>/stat` starttime lookup. Used by
/// `ooda_actions::advance_goal` so it can do the same starttime-validated
/// claim check as the sweep path.
pub fn read_pid_starttime_public(pid: i32) -> Option<u64> {
    read_pid_starttime(pid)
}

/// Read the engineer-claim sentinel out of `worktree_dir/.simard-engineer-claim`.
/// Returns `None` if the file is missing, empty, malformed, or unreadable.
/// Tolerant of all I/O errors — the caller treats `None` as "no claim".
pub fn read_engineer_claim_full(worktree_dir: &Path) -> Option<EngineerClaim> {
    let path = worktree_dir.join(ENGINEER_CLAIM_FILE);
    let raw = fs::read_to_string(&path).ok()?;
    let mut lines = raw.lines();
    let pid: i32 = lines.next()?.trim().parse().ok()?;
    let starttime = lines.next().and_then(|s| s.trim().parse::<u64>().ok());
    Some(EngineerClaim { pid, starttime })
}

/// Back-compat thin wrapper for callers that only care about the PID.
#[allow(dead_code)]
pub fn read_engineer_claim(worktree_dir: &Path) -> Option<i32> {
    read_engineer_claim_full(worktree_dir).map(|c| c.pid)
}

/// Decide whether a parsed claim still names the original allocating process.
///
/// Returns `true` only if BOTH:
///   1. `kill(pid, 0)` reports the PID is alive
///   2. The recorded starttime matches the live process's current starttime
///      (or the claim has no starttime, in which case we fall back to PID-only)
///
/// The starttime check defends against the daemon-restart-with-recycled-PID
/// false positive: after a daemon restart the old PID may eventually be
/// reused by an unrelated process, but its starttime will differ.
pub fn claim_is_live(claim: &EngineerClaim) -> bool {
    if !is_pid_alive(claim.pid) {
        return false;
    }
    match claim.starttime {
        Some(recorded) => match read_pid_starttime(claim.pid) {
            Some(current) => current == recorded,
            // Process exists but we can't read its stat — be conservative
            // and treat as NOT live (better to occasionally re-allocate a
            // worktree than to nuke a live engineer's cwd).
            None => false,
        },
        // Pre-#1238 sentinel: no starttime recorded. Fall back to PID-only.
        None => true,
    }
}
