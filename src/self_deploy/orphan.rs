//! Engineer-orphan reaper: clear stale `bin/simard engineer run …` subprocesses
//! that still hold the old binary's inode open before an atomic swap.
//!
//! Swapping the binary while such a process runs causes **"Text file busy"** and
//! a silent restart of the *old* binary. The match is conservative: a process
//! is an orphan only when its executable path equals the target install path
//! **and** its argv contains the `engineer run` subcommand, excluding the
//! daemon itself and the incoming PID.
//!
//! See `docs/reference/self-deploy-api.md#engineer-orphan-reaper`.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::error::{SimardError, SimardResult};

/// A process matched for reaping: same executable as the daemon binary AND argv
/// contains the `engineer run` subcommand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrphanEngineer {
    pub pid: i32,
    pub cmdline: String,
}

/// Pure matching predicate (the load-bearing decision, fully unit-testable).
///
/// A process is an engineer-orphan of `install_path` when **all** hold:
///   * its executable path equals `install_path`,
///   * its argv contains the `engineer run` subcommand token sequence,
///   * its pid is neither `self_pid` nor `new_daemon_pid`.
pub fn match_engineer_orphan(
    install_path: &Path,
    exe_path: &Path,
    cmdline: &str,
    pid: i32,
    self_pid: i32,
    new_daemon_pid: Option<i32>,
) -> bool {
    if pid == self_pid {
        return false;
    }
    if Some(pid) == new_daemon_pid {
        return false;
    }
    if exe_path != install_path {
        return false;
    }
    cmdline_has_engineer_run(cmdline)
}

/// True when argv contains the adjacent `engineer run` subcommand tokens.
///
/// Whitespace-tokenized so `bin/simard engineer run --goal x` matches but a
/// stray `engineer` or `run` alone does not.
fn cmdline_has_engineer_run(cmdline: &str) -> bool {
    let tokens: Vec<&str> = cmdline.split_whitespace().collect();
    tokens
        .windows(2)
        .any(|w| w[0] == "engineer" && w[1] == "run")
}

/// Scan the live process table for stale engineer subprocesses still bound to
/// `install_path`. Excludes `self_pid` and `new_daemon_pid`.
///
/// Effectful (reads `/proc`). The pure matching rule is [`match_engineer_orphan`].
/// Best-effort per-entry: a process that exits mid-scan (its `/proc/<pid>`
/// vanishing) is skipped rather than failing the whole scan.
pub fn find_engineer_orphans(
    install_path: &Path,
    self_pid: i32,
    new_daemon_pid: Option<i32>,
) -> SimardResult<Vec<OrphanEngineer>> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir("/proc") {
        Ok(e) => e,
        // No /proc (non-Linux / restricted): nothing to reap, not an error.
        Err(_) => return Ok(out),
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid: i32 = match name.to_string_lossy().parse() {
            Ok(p) => p,
            Err(_) => continue, // non-numeric /proc entry
        };

        // Resolve the executable path via /proc/<pid>/exe. Missing/denied ⇒ skip.
        let exe = match std::fs::read_link(format!("/proc/{pid}/exe")) {
            Ok(p) => p,
            Err(_) => continue,
        };

        // /proc/<pid>/cmdline is NUL-separated argv; render it space-joined so
        // the same predicate works on the live table and in unit tests.
        let cmdline = match std::fs::read(format!("/proc/{pid}/cmdline")) {
            Ok(bytes) => cmdline_from_proc(&bytes),
            Err(_) => continue,
        };

        if match_engineer_orphan(install_path, &exe, &cmdline, pid, self_pid, new_daemon_pid) {
            out.push(OrphanEngineer { pid, cmdline });
        }
    }

    Ok(out)
}

/// Decode a `/proc/<pid>/cmdline` NUL-separated argv blob into a single
/// whitespace-joined string suitable for [`match_engineer_orphan`].
fn cmdline_from_proc(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .split('\0')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// SIGTERM each orphan, wait up to `grace_seconds`, then SIGKILL survivors.
/// Numeric PID only (no name-based killers, per repo shell policy).
///
/// Idempotent: an empty match set returns `Ok(0)` without touching any process.
/// Returns the number of orphans handled. If any orphan is still alive after
/// SIGTERM + SIGKILL + the grace window, returns a
/// [`SimardError::VerificationFailed`] whose message embeds the surviving pid
/// (the [`OrphanReapTimeout`](crate::safe_update::SafeUpdateError::OrphanReapTimeout)
/// display), so the orchestrator aborts before the swap rather than risk a
/// "Text file busy" / silent old-binary restart.
pub fn reap_engineer_orphans(
    orphans: &[OrphanEngineer],
    grace_seconds: u64,
) -> SimardResult<usize> {
    if orphans.is_empty() {
        return Ok(0);
    }

    // Phase 1: polite SIGTERM to every still-live orphan.
    for o in orphans {
        if process_alive(o.pid) {
            let _ = send_signal(o.pid, libc::SIGTERM);
        }
    }

    // Phase 2: bounded wait for graceful exit.
    let deadline = Instant::now() + Duration::from_secs(grace_seconds);
    let poll = poll_interval_for(grace_seconds);
    loop {
        if orphans.iter().all(|o| !process_alive(o.pid)) {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(poll);
    }

    // Phase 3: SIGKILL survivors, then a short final wait.
    for o in orphans {
        if process_alive(o.pid) {
            let _ = send_signal(o.pid, libc::SIGKILL);
        }
    }
    let kill_deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if orphans.iter().all(|o| !process_alive(o.pid)) {
            break;
        }
        if Instant::now() >= kill_deadline {
            // A survivor of SIGTERM+SIGKILL is a hard failure: report the first
            // so the orchestrator aborts before the swap.
            if let Some(survivor) = orphans.iter().find(|o| process_alive(o.pid)) {
                return Err(SimardError::VerificationFailed {
                    reason: crate::safe_update::SafeUpdateError::OrphanReapTimeout {
                        pid: survivor.pid,
                    }
                    .to_string(),
                });
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    Ok(orphans.len())
}

/// Numeric-PID signal via `libc::kill`. Per repo shell policy we never shell out
/// to name-based process terminators (mirrors `numeric_kill` in
/// `ooda_actions::advance_goal::spawn`).
fn send_signal(pid: i32, signal: libc::c_int) -> std::io::Result<()> {
    // SAFETY: libc::kill is FFI but well-defined for any pid/signal pair.
    let rc = unsafe { libc::kill(pid, signal) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Whether `pid` is a *running* process holding resources (and thus the old
/// binary's inode). Best-effort on Linux via `/proc/<pid>/stat`; a **zombie**
/// (state `Z`) counts as gone because it has already released every file
/// descriptor and cannot cause "Text file busy". On hosts without `/proc` we
/// fall back to `kill(pid, 0)`.
///
/// Non-positive pids are rejected outright: `kill` with pid `0`/`-1`/`<-1`
/// targets process *groups* or broadcasts, which must never happen in a reaper.
fn process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => !is_zombie_stat(&stat),
        Err(_) => {
            // No /proc entry (Linux: process gone) or non-Linux host. Use a
            // signal-0 existence probe as a portable fallback.
            // SAFETY: kill with signal 0 performs error checking without sending.
            let rc = unsafe { libc::kill(pid, 0) };
            rc == 0
        }
    }
}

/// Parse the state field from `/proc/<pid>/stat` and report whether it is a
/// zombie (`Z`). The `comm` field is parenthesized and may contain spaces and
/// parens, so the state is the first token after the **last** `')'`.
fn is_zombie_stat(stat: &str) -> bool {
    match stat.rfind(')') {
        Some(close) => stat[close + 1..].trim_start().starts_with('Z'),
        None => false,
    }
}

/// Scale the graceful-wait poll interval with the grace window; short windows
/// (tests) poll fast, longer windows poll calmly.
fn poll_interval_for(grace_seconds: u64) -> Duration {
    if grace_seconds == 0 {
        Duration::from_millis(20)
    } else if grace_seconds <= 2 {
        Duration::from_millis(50)
    } else {
        Duration::from_millis(200)
    }
}

#[cfg(test)]
mod stat_tests {
    use super::is_zombie_stat;

    #[test]
    fn detects_zombie_state() {
        // `pid (comm) state ...` — state is the token after the LAST ')'.
        assert!(is_zombie_stat("4242 (simard) Z 1 4242 4242 0 -1 ..."));
    }

    #[test]
    fn running_and_sleeping_are_not_zombies() {
        assert!(!is_zombie_stat("4242 (simard) R 1 4242 ..."));
        assert!(!is_zombie_stat("4242 (simard) S 1 4242 ..."));
    }

    #[test]
    fn comm_with_parens_and_spaces_is_handled() {
        // comm can itself contain ')' and spaces; the LAST ')' delimits it.
        assert!(is_zombie_stat("4242 (sim (ard) proc) Z 1 4242 ..."));
        assert!(!is_zombie_stat("4242 (sim (ard) proc) S 1 4242 ..."));
    }
}
