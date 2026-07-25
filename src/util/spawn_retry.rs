//! Bounded-backoff retry for transient fork/exec failures.
//!
//! See `docs/reference/spawn-retry-api.md`. Under high host concurrency the
//! kernel intermittently rejects a subprocess spawn with a *transient* errno
//! (`ETXTBSY`, `EAGAIN`/`EWOULDBLOCK`, `ENOMEM`) even though the same call
//! succeeds moments later. These helpers retry those — and ONLY those — with a
//! small, bounded, capped backoff, so a load artifact never reddens the
//! self-deploy canary. A genuine failure (missing binary, bad permissions, or
//! a process that spawned then exited non-zero) surfaces unchanged.
//!
//! ## Policy
//!
//! - Classification is by raw errno only (`ETXTBSY`, `EAGAIN`/`EWOULDBLOCK`,
//!   `ENOMEM`); every other error is permanent.
//! - Bounded attempts with a short, capped, exponential backoff. On exhaustion
//!   the LAST `Err` (the real transient errno) is returned. A non-transient
//!   error returns immediately with no retry.

use std::collections::BTreeSet;
use std::future::Future;
use std::io;
use std::sync::Mutex;
use std::time::Duration;

/// Process-global registry of **detached** child PIDs that no owner will
/// `wait()` on, so the OODA daemon's per-cycle reaper knows exactly which
/// children are its responsibility to harvest.
///
/// ## Why a registry instead of `waitpid(-1)`
///
/// The reaper originally called `waitpid(-1, WNOHANG)`, which reaps **any**
/// exited child of the process — including a child that another thread (or a
/// `tokio` task) spawned and is itself about to `wait()`/`try_wait()` on. When
/// the reaper wins that race the owner's wait fails with `ECHILD` ("No child
/// processes"), spuriously failing an otherwise-healthy command. Under
/// `cargo test` (one process, many threads) the reaper tests race hundreds of
/// subprocess-spawning tests over a single shared child table, so `waitpid(-1)`
/// reddened the self-deploy canary; in production it can equally steal a
/// concurrently-running `gh`, `git`, or Bash-tool child.
///
/// The fix is to make the reaper **targeted**: only the genuinely-detached
/// children (subordinate engineers spawned without a `Child` owner, and the
/// detached `simard safe-update` dispatch) register their PID here, and the
/// reaper reaps *only* registered PIDs via `waitpid(pid, WNOHANG)`. Every other
/// child — anything spawned-and-waited by its owner — is never touched, so no
/// coordination lock is needed and no owner ever loses its child to `ECHILD`.
static REAPABLE_PIDS: Mutex<BTreeSet<i32>> = Mutex::new(BTreeSet::new());

/// Register a detached child PID for the process-wide reaper to harvest.
///
/// Call this immediately after spawning a child whose `Child` handle is dropped
/// without `wait()` (a fire-and-forget / detached subprocess). Registration is
/// idempotent. Poison is tolerated so a panicked holder never disables the
/// registry.
pub fn register_reapable_child(pid: u32) {
    REAPABLE_PIDS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(pid as i32);
}

/// Snapshot-and-reap all registered detached child PIDs (non-blocking).
///
/// For each registered PID: `waitpid(pid, WNOHANG)` — if it has exited, count it
/// and drop it from the registry; if it is still running, keep it for a later
/// cycle; if it is already gone (`ECHILD`/error), drop it. Returns the number of
/// children actually reaped this call. Never touches an unregistered child, so
/// it can never steal a child another owner is waiting on.
#[cfg(unix)]
pub fn reap_registered_children() -> usize {
    let mut pids = REAPABLE_PIDS.lock().unwrap_or_else(|p| p.into_inner());
    let mut reaped = 0usize;
    pids.retain(|&pid| {
        let mut status: libc::c_int = 0;
        // SAFETY: `waitpid` on a specific positive PID with WNOHANG is a
        // non-blocking query against the kernel's child table for a PID this
        // process spawned. `status` is a stack local we own.
        let r = unsafe { libc::waitpid(pid, &mut status as *mut libc::c_int, libc::WNOHANG) };
        if r == pid {
            reaped += 1;
            false // exited and reaped — remove
        } else if r == 0 {
            true // still running — keep for a later cycle
        } else {
            false // r == -1: already gone / not our child — remove
        }
    });
    reaped
}

#[cfg(not(unix))]
pub fn reap_registered_children() -> usize {
    0
}

/// Linux errno values that clear on a brief retry. `EAGAIN == EWOULDBLOCK == 11`
/// on Linux, so the single value covers both.
const ETXTBSY: i32 = 26;
const EAGAIN: i32 = 11;
const ENOMEM: i32 = 12;

/// Total spawn attempts (1 initial + up to `MAX_ATTEMPTS - 1` retries).
const MAX_ATTEMPTS: usize = 6;
/// First backoff step; doubles each retry up to [`BACKOFF_CAP`].
const BACKOFF_BASE: Duration = Duration::from_millis(5);
/// Upper bound on any single backoff sleep.
const BACKOFF_CAP: Duration = Duration::from_millis(80);

/// Backoff before the `attempt`-th retry (0-based: retry 0 waits `BACKOFF_BASE`).
fn backoff_for(retry_index: u32) -> Duration {
    let scaled = BACKOFF_BASE
        .checked_mul(1u32 << retry_index.min(16))
        .unwrap_or(BACKOFF_CAP);
    scaled.min(BACKOFF_CAP)
}

/// Transient fork/exec errno values that clear on a brief retry.
///
/// Classification is by **raw errno**, never by matching the message string.
///   - `ETXTBSY`  (26) — "Text file busy": image concurrently written/executed.
///   - `EAGAIN` / `EWOULDBLOCK` (11) — momentary process/memory limit on fork.
///   - `ENOMEM`  (12) — transient allocation failure during fork.
///
/// Returns `false` for every other error, including `ENOENT` (binary not
/// found), permission denied, and any non-OS error — all treated as permanent.
pub fn is_transient_spawn_error(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(ETXTBSY | EAGAIN | ENOMEM))
}

/// Synchronous bounded-backoff spawn retry.
///
/// `f` must **rebuild and launch** the subprocess on each attempt (because
/// `std::process::Command` is not `Clone`). Returns immediately on `Ok`, retries
/// only when [`is_transient_spawn_error`] is `true` up to a bounded attempt
/// count, and returns the last `Err` (the real transient errno) on exhaustion.
/// A non-transient `Err` returns immediately with no retry.
pub fn retry_spawn_sync<T>(mut f: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut retry_index = 0u32;
    loop {
        match f() {
            Ok(value) => return Ok(value),
            Err(e) => {
                let attempts_made = retry_index as usize + 1;
                if attempts_made >= MAX_ATTEMPTS || !is_transient_spawn_error(&e) {
                    return Err(e);
                }
                std::thread::sleep(backoff_for(retry_index));
                retry_index += 1;
            }
        }
    }
}

/// Asynchronous counterpart of [`retry_spawn_sync`] for `tokio` spawns.
///
/// Shares the exact same classifier and bounded-attempt / capped-backoff
/// policy; only the sleep mechanism differs (`tokio` sleep vs blocking sleep).
pub async fn retry_spawn_async<T, Fut>(mut f: impl FnMut() -> Fut) -> io::Result<T>
where
    Fut: Future<Output = io::Result<T>>,
{
    let mut retry_index = 0u32;
    loop {
        match f().await {
            Ok(value) => return Ok(value),
            Err(e) => {
                let attempts_made = retry_index as usize + 1;
                if attempts_made >= MAX_ATTEMPTS || !is_transient_spawn_error(&e) {
                    return Err(e);
                }
                tokio::time::sleep(backoff_for(retry_index)).await;
                retry_index += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Linux errno values. EAGAIN == EWOULDBLOCK == 11.
    const ETXTBSY: i32 = 26;
    const EAGAIN: i32 = 11;
    const ENOMEM: i32 = 12;
    const ENOENT: i32 = 2;
    const EACCES: i32 = 13;

    // ── classifier ──────────────────────────────────────────────────────────

    #[test]
    fn classifier_marks_only_transient_errnos_true() {
        for errno in [ETXTBSY, EAGAIN, ENOMEM] {
            assert!(
                is_transient_spawn_error(&io::Error::from_raw_os_error(errno)),
                "errno {errno} must be classified transient"
            );
        }
    }

    #[test]
    fn classifier_marks_permanent_errors_false() {
        // Permanent OS errors.
        for errno in [ENOENT, EACCES] {
            assert!(
                !is_transient_spawn_error(&io::Error::from_raw_os_error(errno)),
                "errno {errno} must be classified permanent"
            );
        }
        // A non-OS error (no raw_os_error) is permanent.
        let non_os = io::Error::other("synthetic, no errno");
        assert!(
            !is_transient_spawn_error(&non_os),
            "a non-OS error must be classified permanent"
        );
    }

    // ── retry_spawn_sync ─────────────────────────────────────────────────────

    #[test]
    fn sync_returns_immediately_on_success() {
        let attempts = Cell::new(0usize);
        let out = retry_spawn_sync(|| {
            attempts.set(attempts.get() + 1);
            Ok::<u32, io::Error>(99)
        })
        .unwrap();
        assert_eq!(out, 99);
        assert_eq!(attempts.get(), 1, "success must not retry");
    }

    #[test]
    fn sync_retries_transient_then_succeeds() {
        let attempts = Cell::new(0usize);
        let out = retry_spawn_sync(|| {
            let n = attempts.get() + 1;
            attempts.set(n);
            if n < 3 {
                Err(io::Error::from_raw_os_error(EAGAIN))
            } else {
                Ok::<u32, io::Error>(7)
            }
        })
        .unwrap();
        assert_eq!(out, 7);
        assert!(
            attempts.get() >= 3,
            "must have retried through the transient failures; attempts={}",
            attempts.get()
        );
    }

    #[test]
    fn sync_does_not_retry_permanent_error() {
        let attempts = Cell::new(0usize);
        let result: io::Result<u32> = retry_spawn_sync(|| {
            attempts.set(attempts.get() + 1);
            Err(io::Error::from_raw_os_error(ENOENT))
        });
        assert!(result.is_err());
        assert_eq!(
            attempts.get(),
            1,
            "a permanent (non-transient) error must surface on the first attempt"
        );
    }

    #[test]
    fn sync_exhausts_bounded_budget_and_returns_last_transient_err() {
        let attempts = Cell::new(0usize);
        let result: io::Result<u32> = retry_spawn_sync(|| {
            attempts.set(attempts.get() + 1);
            Err(io::Error::from_raw_os_error(ETXTBSY))
        });
        let err = result.expect_err("persistent transient error must eventually give up");
        assert_eq!(
            err.raw_os_error(),
            Some(ETXTBSY),
            "the returned error must be the real last transient errno, not a synthetic one"
        );
        assert!(
            attempts.get() > 1,
            "must have retried before giving up; attempts={}",
            attempts.get()
        );
    }

    // ── retry_spawn_async ────────────────────────────────────────────────────

    #[tokio::test]
    async fn async_retries_transient_then_succeeds() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let a = Arc::clone(&attempts);
        let out = retry_spawn_async(move || {
            let a = Arc::clone(&a);
            async move {
                let n = a.fetch_add(1, Ordering::SeqCst) + 1;
                if n < 3 {
                    Err(io::Error::from_raw_os_error(EAGAIN))
                } else {
                    Ok::<u32, io::Error>(13)
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(out, 13);
        assert!(attempts.load(Ordering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn async_does_not_retry_permanent_error() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let a = Arc::clone(&attempts);
        let result: io::Result<u32> = retry_spawn_async(move || {
            let a = Arc::clone(&a);
            async move {
                a.fetch_add(1, Ordering::SeqCst);
                Err(io::Error::from_raw_os_error(ENOENT))
            }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "permanent error must not be retried"
        );
    }

    #[tokio::test]
    async fn async_exhausts_bounded_budget_and_returns_last_transient_err() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let a = Arc::clone(&attempts);
        let result: io::Result<u32> = retry_spawn_async(move || {
            let a = Arc::clone(&a);
            async move {
                a.fetch_add(1, Ordering::SeqCst);
                Err(io::Error::from_raw_os_error(ENOMEM))
            }
        })
        .await;
        let err = result.expect_err("persistent transient error must eventually give up");
        assert_eq!(err.raw_os_error(), Some(ENOMEM));
        assert!(attempts.load(Ordering::SeqCst) > 1);
    }
}
