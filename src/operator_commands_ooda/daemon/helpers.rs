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

/// Return the mtime of the currently-running executable, or `None` if it
/// cannot be determined (e.g. the binary was deleted after launch).
pub fn exe_mtime() -> Option<SystemTime> {
    std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
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
