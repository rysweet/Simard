use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

/// Append a timestamped log line to `{state_root}/ooda.log` **and** stderr.
///
/// The dashboard `/api/logs` endpoint already looks for `ooda.log` inside the
/// state root, so writing here makes daemon output visible in the Logs tab
/// without requiring systemd or manual redirection.  Failures to write are
/// silently ignored — stderr is the primary output channel.
pub fn daemon_log(state_root: &std::path::Path, msg: &str) {
    let line = format!("{} {msg}", chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ"),);
    eprintln!("{msg}");
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_root.join("ooda.log"))
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Default OODA cycle interval (seconds) when `SIMARD_OODA_INTERVAL_SECS` is
/// unset, empty, non-numeric, or `0`. A `0`/absent interval would busy-loop the
/// daemon — running full OODA cycles back-to-back with no pause, because
/// `interruptible_sleep(Duration::ZERO, …)` returns immediately — which is never
/// intended. Mirrors `backup::backup_interval_secs_from_env`.
pub const DEFAULT_OODA_INTERVAL_SECS: u64 = 300;

/// Parse `SIMARD_OODA_INTERVAL_SECS` into a SAFE cycle interval. A parseable
/// value **> 0** is honoured (leading/trailing whitespace trimmed); anything
/// else — unset, empty, non-numeric, or `0` — falls back to
/// [`DEFAULT_OODA_INTERVAL_SECS`]. Clamping `0` is what prevents the zero-delay
/// busy loop (an oversized value merely sleeps a long time and is harmless, so it
/// is intentionally left unclamped).
pub fn ooda_interval_secs_from_env(raw: Option<&str>) -> u64 {
    match raw.and_then(|v| v.trim().parse::<u64>().ok()) {
        Some(n) if n > 0 => n,
        _ => DEFAULT_OODA_INTERVAL_SECS,
    }
}

/// Total attempts for [`exe_mtime_resolved`]: 1 initial + 4 retries. A fixed,
/// caller-independent budget so no input can drive the loop count (no
/// amplification/DoS surface). Worst-case added latency is the sum of the four
/// between-attempt backoffs (~2 + 4 + 8 + 16 = 30 ms), paid only when every
/// attempt hits a transient error; the steady state succeeds on attempt 1 with
/// zero added latency.
const EXE_MTIME_MAX_ATTEMPTS: u32 = 5;

/// One resolution attempt with **no error swallowing**:
/// `current_exe()? -> metadata()? -> modified()?`. The io error kind flows out
/// so [`exe_mtime_resolved`] can classify transient vs. genuine failures.
fn try_exe_mtime_once() -> std::io::Result<SystemTime> {
    let path = std::env::current_exe()?;
    let meta = std::fs::metadata(path)?;
    meta.modified()
}

/// Whether an io error is a *transient load blip* worth a bounded retry:
/// `EINTR` (`Interrupted`), `EAGAIN`/`EWOULDBLOCK` (`WouldBlock`), and the
/// fd-exhaustion errnos `EMFILE`/`ENFILE`. These are scheduler/resource
/// pressure symptoms under a storm of concurrent test binaries. `NotFound` is
/// deliberately NOT in this set — it is not a load-blip errno; the distinct
/// atomic-replace window is classified by [`is_atomic_replace_window_error`].
/// `PermissionDenied` and every unclassified error stay genuine (never retried),
/// so a real "binary tampered / permission revoked" signal is never masked.
fn is_transient_exe_mtime_error(err: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    // EMFILE (per-process fd table full) and ENFILE (system-wide fd table full)
    // have no dedicated ErrorKind on stable Rust; match them by raw errno.
    const EMFILE: i32 = 24;
    const ENFILE: i32 = 23;
    if matches!(err.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) {
        return true;
    }
    matches!(err.raw_os_error(), Some(EMFILE) | Some(ENFILE))
}

/// Whether an io error is a `NotFound` observed on the current-exe path — the
/// signature of a **transient atomic-replace window**, not necessarily a genuine
/// deletion. During a rebuild/self-deploy the fresh image is `rename(2)`d over
/// the on-disk path, and a `stat(2)` that lands in that microsecond-wide unlink
/// window returns `ENOENT`. This is the root cause of the self-deploy `unit-test`
/// canary going red (exit 101) under the load of a concurrent build replacing
/// the binary: the happy-path `exe_mtime()` calls coerced that blip to a false
/// `None`.
///
/// Retrying `NotFound` is safe and does **not** weaken the fail-closed posture:
/// a *genuine* deletion PERSISTS across the entire bounded-retry budget and still
/// resolves to `None`, while a *transient* replace window heals to `Some(mtime)`
/// on a subsequent attempt. Tamper is separately caught by the content-hash gate
/// in [`reload_decision`], never by mtime alone.
fn is_atomic_replace_window_error(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::NotFound
}

/// Whether a failed attempt should be re-resolved within the bounded budget:
/// a transient load blip ([`is_transient_exe_mtime_error`]) OR a transient
/// atomic-replace window ([`is_atomic_replace_window_error`]). Every other error
/// (notably `PermissionDenied` and unclassified) is genuine and short-circuits.
fn is_retryable_exe_mtime_error(err: &std::io::Error) -> bool {
    is_transient_exe_mtime_error(err) || is_atomic_replace_window_error(err)
}

/// Bounded-retry resolver over [`try_exe_mtime_once`]. Retries only retryable
/// errors ([`is_retryable_exe_mtime_error`]) up to [`EXE_MTIME_MAX_ATTEMPTS`]
/// with exponential backoff (~2, 4, 8, 16 ms). Sleeps happen **between** attempts
/// only — never after the terminal attempt — so the fail-closed `None` is not
/// needlessly delayed. Genuine errors short-circuit to `Err` with no retry.
/// Returns the resolved mtime, or the last error on exhaustion (with one
/// structured `tracing::warn!`).
fn exe_mtime_resolved() -> std::io::Result<SystemTime> {
    resolve_exe_mtime_with(try_exe_mtime_once)
}

/// Pure bounded-retry engine, generic over the per-attempt resolver so the
/// retry/backoff/fail-closed policy is unit-testable with synthetic error
/// sequences (no real syscalls). See [`exe_mtime_resolved`] for the production
/// wiring. Tests that heal on an early attempt incur no backoff; the
/// worst-case (a persistent retryable error) pays only the tiny fixed budget
/// (~30 ms total), keeping the deterministic tests fast.
fn resolve_exe_mtime_with<F>(mut attempt_once: F) -> std::io::Result<SystemTime>
where
    F: FnMut() -> std::io::Result<SystemTime>,
{
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..EXE_MTIME_MAX_ATTEMPTS {
        match attempt_once() {
            Ok(mtime) => return Ok(mtime),
            Err(err) => {
                if !is_retryable_exe_mtime_error(&err) {
                    // Genuine failure (PermissionDenied/unclassified): do not
                    // swallow, do not retry — fail-closed straight through.
                    return Err(err);
                }
                let is_last_attempt = attempt + 1 == EXE_MTIME_MAX_ATTEMPTS;
                last_err = Some(err);
                if is_last_attempt {
                    break;
                }
                // Backoff BETWEEN attempts only: ~2, 4, 8, 16 ms.
                std::thread::sleep(Duration::from_millis(2u64 << attempt));
            }
        }
    }
    let err = last_err.unwrap_or_else(|| std::io::Error::other("exe_mtime: retries exhausted"));
    // At most one structured line on exhaustion — attempt count + errno only,
    // never the executable path or any sensitive payload.
    tracing::warn!(
        attempts = EXE_MTIME_MAX_ATTEMPTS,
        errno = err.raw_os_error(),
        "exe_mtime: transient resolution failed after bounded retries; failing closed to None"
    );
    Err(err)
}

/// Return the mtime of the currently-running executable, or `None` if it
/// genuinely cannot be determined (binary permanently deleted, permission
/// denied, or a filesystem that does not report mtime). A *transient* failure
/// under load — a load-blip errno (`EINTR`/`EAGAIN`/`EMFILE`/`ENFILE`) or the
/// microsecond-wide `NotFound` atomic-replace window when a concurrent
/// rebuild/self-deploy `rename(2)`s a fresh image over the on-disk path — is
/// re-resolved within a small bounded budget and yields `Some(mtime)` rather
/// than a false `None`. Never panics.
///
/// Public signature is unchanged (`-> Option<SystemTime>`); only the transient
/// case changed (it no longer coerces to `None`). A *persistent* failure still
/// fails closed to `None`, and [`binary_changed`] stays fail-closed on it.
pub fn exe_mtime() -> Option<SystemTime> {
    exe_mtime_resolved().ok()
}

/// Whether the on-disk binary is a **genuinely different image** than the one
/// this process is running — the gate for an auto-reload `exec()`.
///
/// This replaces the historical mtime-only check (`exe_mtime() > start_time`),
/// which relaunched on *any* rebuild/`touch` even when the resulting binary was
/// byte-for-byte identical. On a host that periodically rebuilds the daemon from
/// an unchanged tree, that turned every ~40–45 min into a full cold-start `exec`
/// (slow recall/preparation each time) for no benefit (2026-07-02
/// operator-review #2). The gate now confirms a real content difference before
/// paying for a relaunch:
///
/// 1. **mtime pre-filter** — if the on-disk mtime is not newer than the image we
///    started from, nothing changed; return `false` without hashing (the
///    steady-state hot path, run every cycle).
/// 2. **content confirmation** — only when the mtime IS newer do we hash the
///    on-disk file and compare it to the [`running_image_hash`]. Identical
///    content (a no-op rebuild) → `false`; a different digest → `true`.
///
/// Fail-closed throughout: an unreadable mtime, an unhashable on-disk file, or an
/// unknown running identity all yield `false`, because a transient I/O error must
/// never trigger a needless cold start.
pub fn binary_changed(start_time: SystemTime) -> bool {
    let running_hash = match running_image_hash() {
        Some(h) => h,
        // Running identity unknown (couldn't hash our own image at startup) →
        // fail-closed: never relaunch on a guess.
        None => return false,
    };
    // mtime pre-filter FIRST, so the expensive on-disk content hash is NOT
    // computed on the steady-state hot path (this runs every cycle). Only when
    // the mtime has actually advanced do we read + SHA-256 the (multi-MB) binary.
    let on_disk_mtime = exe_mtime();
    if !mtime_advanced(start_time, on_disk_mtime) {
        return false;
    }
    let on_disk_hash = current_exe_path().as_deref().and_then(file_content_hash);
    reload_decision(
        start_time,
        on_disk_mtime,
        on_disk_hash.as_deref(),
        running_hash,
    )
}

/// Whether the on-disk mtime is strictly newer than the image we started from —
/// the cheap pre-filter that keeps the multi-MB content hash off the
/// steady-state hot path. `None` (unreadable mtime) is fail-closed to `false`.
///
/// This is the same condition [`reload_decision`] applies internally; extracting
/// it lets [`binary_changed`] skip computing the on-disk hash entirely when it is
/// provably unnecessary (and lets that hot-path gate be unit-tested directly).
fn mtime_advanced(start_time: SystemTime, on_disk_mtime: Option<SystemTime>) -> bool {
    on_disk_mtime.is_some_and(|mtime| mtime > start_time)
}

// ── Wave 2: binary-identity reload gate (2026-07-02 operator-review #2) ──────
//
// The mtime-only `binary_changed` above was the ~40–45 min self-restart churn
// trigger: any rebuild/`touch` bumped the mtime and forced a full `exec()`
// cold-start even when the on-disk image was byte-identical. The two seams below
// (`file_content_hash` + the pure `reload_decision`) let the gate confirm a REAL
// image difference before relaunching, and are unit-tested in isolation. See
// `docs/reference/ooda-binary-identity-reload-gate.md`.

/// Content hash of the RUNNING process image, captured once (lazily) and cached
/// for the life of the process.
///
/// Capturing it once — rather than re-hashing `current_exe()` on every check —
/// is what makes the gate correct: after an in-place replace, `current_exe()`
/// resolves to the NEW bytes, so only a value pinned at (or near) startup still
/// identifies the OLD image we are actually running.
static RUNNING_IMAGE_HASH: OnceLock<Option<String>> = OnceLock::new();

/// Hash of the running image (see [`RUNNING_IMAGE_HASH`]), or `None` if our own
/// executable could not be hashed at capture time. Lazily initialised on first
/// use so unit tests (and any early caller) observe a consistent value.
pub fn running_image_hash() -> Option<&'static str> {
    RUNNING_IMAGE_HASH
        .get_or_init(|| current_exe_path().as_deref().and_then(file_content_hash))
        .as_deref()
}

/// Pin the running-image hash at a known-early point (daemon startup),
/// narrowing the window in which an in-place replace between `exec` and this
/// capture could otherwise be mistaken for the running image (it does not fully
/// close it — the hash is read from the on-disk path — but the effect is
/// bounded: the next genuinely-different rebuild reloads). Idempotent.
pub fn capture_running_image_hash() {
    let _ = running_image_hash();
}

/// The path to this process's executable, or `None` if it cannot be resolved.
fn current_exe_path() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

/// Stable content identity (hex SHA-256 digest) of the file at `path`, or `None`
/// on any I/O error (fail-closed). Never panics — safe to call on the untrusted
/// on-disk binary every cycle. Streams the file through the hasher so a
/// multi-megabyte binary is never fully buffered in memory.
pub fn file_content_hash(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    Some(format!("{:x}", hasher.finalize()))
}

/// Pure reload decision: `true` **only** when the on-disk image is a genuinely
/// different binary than the running one.
///
/// * `on_disk_mtime` not newer than `start_time` → `false` (cheap mtime
///   pre-filter; the content hash is not consulted — the steady-state hot path).
/// * mtime newer **and** `on_disk_hash == Some(running_hash)` → `false`
///   (identical-content rebuild — the churn case this gate eliminates).
/// * mtime newer **and** `on_disk_hash == Some(other)` → `true` (real image
///   difference → relaunch).
/// * mtime newer **and** `on_disk_hash == None` (read/hash error) → `false`
///   (fail-closed — a transient I/O error must never trigger a cold start).
/// * `on_disk_mtime == None` (unreadable) → `false` (fail-closed).
pub fn reload_decision(
    start_time: SystemTime,
    on_disk_mtime: Option<SystemTime>,
    on_disk_hash: Option<&str>,
    running_hash: &str,
) -> bool {
    // Cheap mtime pre-filter — no hashing on the steady-state hot path.
    match on_disk_mtime {
        Some(mtime) if mtime > start_time => {}
        _ => return false,
    }
    // mtime is newer: relaunch only on a confirmed content difference.
    match on_disk_hash {
        Some(hash) => hash != running_hash,
        None => false,
    }
}

/// Replace the current process with a fresh copy of itself.
///
/// On success this function never returns — the process image is replaced
/// via `exec()`.  On failure the error is returned so the caller can
/// degrade gracefully and continue running.
#[cfg(unix)]
pub fn exec_self_reload() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::process::CommandExt;

    let exe = std::env::current_exe()?;
    let args: Vec<String> = std::env::args().skip(1).collect();

    eprintln!("[simard] New binary detected, restarting...");

    // Flush stderr/stdout so the log line above is not lost.
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    let err = std::process::Command::new(&exe).args(&args).exec();
    // exec() only returns on failure
    Err(format!("exec failed: {err}").into())
}

/// Sleep that wakes early when the shutdown flag is set.
pub fn interruptible_sleep(total: Duration, shutdown: &AtomicBool) {
    let tick = Duration::from_millis(250);
    let mut remaining = total;
    while remaining > Duration::ZERO {
        if shutdown.load(Ordering::Relaxed) {
            return;
        }
        let chunk = remaining.min(tick);
        std::thread::sleep(chunk);
        remaining = remaining.saturating_sub(chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    // ── daemon_log ──────────────────────────────────────────────────

    #[test]
    fn daemon_log_creates_file_and_writes_message() {
        let dir = tempfile::tempdir().unwrap();
        daemon_log(dir.path(), "hello from test");
        let contents = std::fs::read_to_string(dir.path().join("ooda.log")).unwrap();
        assert!(contents.contains("hello from test"));
    }

    #[test]
    fn daemon_log_appends_multiple_lines() {
        let dir = tempfile::tempdir().unwrap();
        daemon_log(dir.path(), "line-one");
        daemon_log(dir.path(), "line-two");
        let contents = std::fs::read_to_string(dir.path().join("ooda.log")).unwrap();
        assert!(contents.contains("line-one"));
        assert!(contents.contains("line-two"));
        let line_count = contents.lines().count();
        assert!(
            line_count >= 2,
            "should have at least 2 lines, got {line_count}"
        );
    }

    #[test]
    fn daemon_log_includes_iso_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        daemon_log(dir.path(), "ts-check");
        let contents = std::fs::read_to_string(dir.path().join("ooda.log")).unwrap();
        // ISO 8601: contains 'T' and 'Z'
        assert!(
            contents.contains('T') && contents.contains('Z'),
            "expected ISO timestamp in log line, got: {contents}"
        );
    }

    #[test]
    fn daemon_log_survives_missing_directory() {
        // Writing to a nonexistent directory should not panic — the eprintln
        // call still succeeds and the file write is silently ignored.
        let bad_path = std::path::Path::new("/tmp/nonexistent-ooda-test-dir-12345");
        daemon_log(bad_path, "should not panic");
        // No assertion needed — just verifying no panic.
    }

    // ── exe_mtime ───────────────────────────────────────────────────

    #[test]
    fn exe_mtime_returns_some_for_running_binary() {
        assert!(exe_mtime().is_some(), "test binary must have a valid mtime");
    }

    #[test]
    fn exe_mtime_is_in_the_past() {
        let mtime = exe_mtime().unwrap();
        let elapsed = mtime.elapsed().unwrap_or(Duration::ZERO);
        assert!(
            elapsed < Duration::from_secs(365 * 86400),
            "binary should have been built within the last year"
        );
    }

    // ── exe_mtime transient-resilience contract (deterministic) ─────
    //
    // These pin the *mechanism* that makes `exe_mtime()` deterministic under
    // heavy parallel load — the exit-101 red-canary fix. They are load-
    // independent (they construct synthetic io errors and inspect a fixed
    // budget), so unlike the happy-path `exe_mtime_returns_some_*` tests they
    // cannot flake regardless of scheduling. The security invariant they
    // encode: only allow-listed *transient* errnos are retried; every genuine
    // failure short-circuits (fail-closed), so a real "binary missing/tampered"
    // signal is never masked as a load blip.

    #[test]
    fn transient_classifier_retries_eintr() {
        let err = std::io::Error::from(std::io::ErrorKind::Interrupted);
        assert!(
            is_transient_exe_mtime_error(&err),
            "EINTR (Interrupted) must be treated as transient and retried"
        );
    }

    #[test]
    fn transient_classifier_retries_eagain_ewouldblock() {
        let err = std::io::Error::from(std::io::ErrorKind::WouldBlock);
        assert!(
            is_transient_exe_mtime_error(&err),
            "EAGAIN/EWOULDBLOCK (WouldBlock) must be treated as transient and retried"
        );
    }

    #[test]
    fn transient_classifier_retries_emfile_fd_exhaustion() {
        // EMFILE (24): per-process fd table full — the classic symptom under a
        // storm of concurrent test binaries. Must be transient/retryable.
        const EMFILE: i32 = 24;
        let err = std::io::Error::from_raw_os_error(EMFILE);
        assert!(
            is_transient_exe_mtime_error(&err),
            "EMFILE (per-process fd exhaustion) must be treated as transient"
        );
    }

    #[test]
    fn transient_classifier_retries_enfile_fd_exhaustion() {
        // ENFILE (23): system-wide fd table full. Must be transient/retryable.
        const ENFILE: i32 = 23;
        let err = std::io::Error::from_raw_os_error(ENFILE);
        assert!(
            is_transient_exe_mtime_error(&err),
            "ENFILE (system-wide fd exhaustion) must be treated as transient"
        );
    }

    #[test]
    fn transient_classifier_excludes_not_found_from_load_blips() {
        // NotFound is NOT a load-blip errno: `is_transient_exe_mtime_error` must
        // return false for it. It is instead classified as a transient
        // atomic-replace window by `is_atomic_replace_window_error` (asserted
        // separately below) — the two concerns are kept distinct on purpose.
        let err = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(
            !is_transient_exe_mtime_error(&err),
            "NotFound must not be classified as a load-blip errno"
        );
    }

    #[test]
    fn replace_window_classifier_retries_not_found() {
        // The exit-101 self-deploy-canary fix: a NotFound on the current-exe
        // path is the signature of a `rename(2)` atomic-replace window during a
        // concurrent rebuild, so it MUST be re-resolved (retryable). A genuine
        // deletion still fails closed by *persisting* across the whole budget.
        let err = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(
            is_atomic_replace_window_error(&err),
            "NotFound (atomic-replace window) must be retryable"
        );
        assert!(
            is_retryable_exe_mtime_error(&err),
            "NotFound must be retried by the resolver"
        );
    }

    #[test]
    fn replace_window_classifier_excludes_permission_denied() {
        // Only NotFound is a replace window; PermissionDenied is genuine.
        let err = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(
            !is_atomic_replace_window_error(&err),
            "PermissionDenied is not an atomic-replace window"
        );
    }

    #[test]
    fn transient_classifier_does_not_retry_permission_denied() {
        // SECURITY: PermissionDenied is a genuine failure, not a load blip, and
        // not a replace window — it must never be retried.
        let err = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(
            !is_transient_exe_mtime_error(&err),
            "PermissionDenied must NOT be a load-blip errno"
        );
        assert!(
            !is_retryable_exe_mtime_error(&err),
            "PermissionDenied must NOT be retried — fail closed"
        );
    }

    // ── resolver policy (deterministic, synthetic error sequences) ──────
    //
    // These drive `resolve_exe_mtime_with` with in-memory attempt sequences so
    // the retry/heal/fail-closed policy is proven WITHOUT touching real syscalls
    // or depending on scheduling — the load-independent counterpart to the
    // happy-path `exe_mtime_returns_some_*` tests.

    fn err_kind(kind: std::io::ErrorKind) -> std::io::Error {
        std::io::Error::from(kind)
    }

    #[test]
    fn resolver_heals_transient_replace_window_not_found() {
        // A single NotFound (atomic-replace window) followed by success must
        // resolve to Some — the exact production blip that was flaking the
        // self-deploy unit-test canary red (exit 101).
        let want = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let seq = std::cell::Cell::new(0u32);
        let out = resolve_exe_mtime_with(|| {
            let n = seq.get();
            seq.set(n + 1);
            if n == 0 {
                Err(err_kind(std::io::ErrorKind::NotFound))
            } else {
                Ok(want)
            }
        });
        assert_eq!(out.ok(), Some(want), "replace-window NotFound must heal");
    }

    #[test]
    fn resolver_heals_transient_load_blip_then_success() {
        // EINTR then success also heals within the budget.
        let want = SystemTime::UNIX_EPOCH + Duration::from_secs(42);
        let seq = std::cell::Cell::new(0u32);
        let out = resolve_exe_mtime_with(|| {
            let n = seq.get();
            seq.set(n + 1);
            if n < 2 {
                Err(err_kind(std::io::ErrorKind::Interrupted))
            } else {
                Ok(want)
            }
        });
        assert_eq!(out.ok(), Some(want));
    }

    #[test]
    fn resolver_fails_closed_on_persistent_not_found() {
        // SECURITY / fail-closed: a genuine deletion returns NotFound on EVERY
        // attempt, so the resolver exhausts the budget and yields Err (→ None).
        // This is what preserves the "real missing binary is never masked"
        // invariant even though NotFound is now retryable.
        let attempts = std::cell::Cell::new(0u32);
        let out = resolve_exe_mtime_with(|| {
            attempts.set(attempts.get() + 1);
            Err(err_kind(std::io::ErrorKind::NotFound))
        });
        assert!(
            out.is_err(),
            "persistent NotFound must fail closed to Err/None"
        );
        assert_eq!(
            attempts.get(),
            EXE_MTIME_MAX_ATTEMPTS,
            "a persistent retryable error must consume the full bounded budget"
        );
    }

    #[test]
    fn resolver_short_circuits_permission_denied_without_retry() {
        // A genuine (non-retryable) error must short-circuit on the FIRST
        // attempt — no retries, no masking.
        let attempts = std::cell::Cell::new(0u32);
        let out = resolve_exe_mtime_with(|| {
            attempts.set(attempts.get() + 1);
            Err(err_kind(std::io::ErrorKind::PermissionDenied))
        });
        assert!(out.is_err(), "PermissionDenied must fail closed");
        assert_eq!(
            attempts.get(),
            1,
            "a genuine error must not be retried (exactly one attempt)"
        );
    }

    #[test]
    fn resolver_returns_first_attempt_success_without_retry() {
        // Steady-state hot path: success on attempt 1, no extra attempts.
        let want = SystemTime::UNIX_EPOCH + Duration::from_secs(7);
        let attempts = std::cell::Cell::new(0u32);
        let out = resolve_exe_mtime_with(|| {
            attempts.set(attempts.get() + 1);
            Ok(want)
        });
        assert_eq!(out.ok(), Some(want));
        assert_eq!(
            attempts.get(),
            1,
            "steady state resolves on the first attempt"
        );
    }

    #[test]
    fn transient_classifier_is_conservative_on_unclassified_errors() {
        // Default posture is do-not-swallow: anything not on the transient
        // allow-list (here a generic Other error) must be treated as genuine.
        let err = std::io::Error::other("unclassified");
        assert!(
            !is_transient_exe_mtime_error(&err),
            "unclassified errors must default to NON-transient (conservative)"
        );
    }

    #[test]
    fn exe_mtime_retry_budget_is_bounded_and_small() {
        // The budget must be a fixed, caller-independent constant so no input
        // can drive the loop count (no amplification/DoS surface) and the
        // worst-case added latency stays tiny. 1 initial + 4 retries = 5.
        // `== 5` already guarantees the `>= 1` "at least one attempt" invariant,
        // so a separate `assert!(EXE_MTIME_MAX_ATTEMPTS >= 1)` would be a
        // constant-value assertion (clippy::assertions_on_constants) with no
        // added coverage.
        assert_eq!(
            EXE_MTIME_MAX_ATTEMPTS, 5,
            "retry budget must be exactly 5 (1 initial + 4 retries)"
        );
    }

    #[test]
    fn exe_mtime_resolved_is_ok_for_running_binary() {
        // The internal resolver — the seam `exe_mtime()` delegates to — must
        // succeed for the live test binary in steady state.
        assert!(
            exe_mtime_resolved().is_ok(),
            "resolver must produce a value for the running binary"
        );
    }

    #[test]
    fn exe_mtime_matches_resolver_ok_value() {
        // `exe_mtime()` is exactly `exe_mtime_resolved().ok()` — the transient
        // case no longer coerces to a false None. When resolution succeeds the
        // two must agree, proving no lossy coercion sits between them.
        let resolved = exe_mtime_resolved().ok();
        let public = exe_mtime();
        assert_eq!(
            resolved.is_some(),
            public.is_some(),
            "exe_mtime() and exe_mtime_resolved().ok() must agree on presence"
        );
        if let (Some(a), Some(b)) = (resolved, public) {
            assert_eq!(a, b, "the two seams must resolve the identical mtime");
        }
    }

    #[test]
    fn exe_mtime_is_stable_under_repeated_calls() {
        // Regression guard for the exit-101 canary: in steady state the running
        // binary always resolves, so a tight burst of calls must be uniformly
        // Some (never a transient false None). Deterministic in-process.
        for i in 0..256 {
            assert!(
                exe_mtime().is_some(),
                "exe_mtime() returned None on iteration {i}; transient coercion regressed"
            );
        }
    }

    // ── binary_changed ──────────────────────────────────────────────

    #[test]
    fn binary_changed_false_when_start_time_is_now() {
        assert!(!binary_changed(SystemTime::now()));
    }

    /// Wave 2 contract (RED against today's mtime-only `binary_changed`): an
    /// **ancient** start time must NOT trigger a reload when the on-disk binary
    /// is *identical content* to the running one — which is exactly the case
    /// here, because the test binary IS `current_exe()` and nothing rebuilds it
    /// mid-test. mtime-only returns `true` (the ~40-min churn bug); the
    /// content-identity gate must return `false` (no relaunch on identical
    /// content). This is the public-API expression of the churn fix.
    #[test]
    fn binary_changed_false_on_identical_content_even_with_ancient_start_time() {
        assert!(
            !binary_changed(SystemTime::UNIX_EPOCH),
            "identical on-disk content must not relaunch, even with an epoch start time"
        );
    }

    #[test]
    fn binary_changed_false_when_start_time_is_far_future() {
        let future = SystemTime::now() + Duration::from_secs(86400 * 365 * 10);
        assert!(!binary_changed(future));
    }

    // ── file_content_hash (Wave 2 seam) ─────────────────────────────

    #[test]
    fn file_content_hash_is_stable_for_identical_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        let b = dir.path().join("b.bin");
        std::fs::write(&a, b"identical bytes \x00\x01\x02").unwrap();
        std::fs::write(&b, b"identical bytes \x00\x01\x02").unwrap();
        let ha = file_content_hash(&a).expect("readable file must hash");
        let hb = file_content_hash(&b).expect("readable file must hash");
        assert_eq!(ha, hb, "identical bytes must produce an identical hash");
    }

    #[test]
    fn file_content_hash_differs_for_different_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        let b = dir.path().join("b.bin");
        std::fs::write(&a, b"one payload").unwrap();
        std::fs::write(&b, b"another payload").unwrap();
        assert_ne!(
            file_content_hash(&a).unwrap(),
            file_content_hash(&b).unwrap(),
            "different bytes must produce different hashes"
        );
    }

    #[test]
    fn file_content_hash_is_none_on_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.bin");
        assert!(
            file_content_hash(&missing).is_none(),
            "a missing/unreadable file must hash to None (fail-closed upstream)"
        );
    }

    // ── reload_decision (Wave 2 seam) ───────────────────────────────

    const RUNNING: &str = "hash-of-the-running-image";

    #[test]
    fn reload_decision_false_when_mtime_not_newer() {
        let start = SystemTime::now();
        let older = start - Duration::from_secs(60);
        // mtime not newer than start ⇒ pre-filter returns false WITHOUT consulting
        // the hash (a differing hash here must be ignored on the hot path).
        assert!(!reload_decision(
            start,
            Some(older),
            Some("some-other-hash"),
            RUNNING
        ));
    }

    #[test]
    fn reload_decision_false_on_identical_content_rebuild() {
        // Newer mtime BUT identical content hash ⇒ the churn case ⇒ no relaunch.
        let start = SystemTime::now();
        let newer = start + Duration::from_secs(60);
        assert!(!reload_decision(start, Some(newer), Some(RUNNING), RUNNING));
    }

    #[test]
    fn reload_decision_true_on_genuinely_different_image() {
        // Newer mtime AND different content hash ⇒ a real new image ⇒ relaunch.
        let start = SystemTime::now();
        let newer = start + Duration::from_secs(60);
        assert!(reload_decision(
            start,
            Some(newer),
            Some("a-genuinely-different-hash"),
            RUNNING
        ));
    }

    #[test]
    fn reload_decision_fail_closed_when_on_disk_hash_unreadable() {
        // Newer mtime but the on-disk hash could not be computed ⇒ fail-closed
        // (do NOT relaunch on a transient read/hash error).
        let start = SystemTime::now();
        let newer = start + Duration::from_secs(60);
        assert!(!reload_decision(start, Some(newer), None, RUNNING));
    }

    #[test]
    fn reload_decision_fail_closed_when_mtime_unreadable() {
        // Unreadable mtime ⇒ fail-closed regardless of the hashes.
        assert!(!reload_decision(
            SystemTime::now(),
            None,
            Some("a-different-hash"),
            RUNNING
        ));
    }

    // ── mtime_advanced (hot-path gate) ──────────────────────────────

    #[test]
    fn mtime_advanced_false_when_not_newer_so_hash_is_skipped() {
        // The gate `binary_changed` uses to AVOID hashing the multi-MB binary on
        // the steady-state hot path: an mtime not newer than start ⇒ no hash.
        let start = SystemTime::now();
        let older = start - Duration::from_secs(60);
        assert!(!mtime_advanced(start, Some(older)));
        assert!(!mtime_advanced(start, Some(start)));
    }

    #[test]
    fn mtime_advanced_true_only_when_strictly_newer() {
        let start = SystemTime::now();
        let newer = start + Duration::from_secs(60);
        assert!(mtime_advanced(start, Some(newer)));
    }

    #[test]
    fn mtime_advanced_false_when_mtime_unreadable() {
        // Fail-closed: an unreadable mtime must not provoke a content hash.
        assert!(!mtime_advanced(SystemTime::now(), None));
    }

    // ── interruptible_sleep ─────────────────────────────────────────

    #[test]
    fn interruptible_sleep_zero_duration_returns_immediately() {
        let shutdown = AtomicBool::new(false);
        let start = Instant::now();
        interruptible_sleep(Duration::ZERO, &shutdown);
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn interruptible_sleep_completes_short_sleep() {
        let shutdown = AtomicBool::new(false);
        let start = Instant::now();
        interruptible_sleep(Duration::from_millis(100), &shutdown);
        assert!(start.elapsed() >= Duration::from_millis(100));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn interruptible_sleep_exits_immediately_when_already_shutdown() {
        let shutdown = AtomicBool::new(true);
        let start = Instant::now();
        interruptible_sleep(Duration::from_secs(60), &shutdown);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn interruptible_sleep_exits_on_mid_sleep_shutdown() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&shutdown);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            flag.store(true, Ordering::SeqCst);
        });
        let start = Instant::now();
        interruptible_sleep(Duration::from_secs(60), &shutdown);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "should wake within ~350ms of shutdown signal, not wait 60s"
        );
    }
}
