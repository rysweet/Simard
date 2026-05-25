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

/// Return the mtime of the currently-running executable, or `None` if it
/// cannot be determined (e.g. the binary was deleted after launch).
pub fn exe_mtime() -> Option<SystemTime> {
    std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
}

/// Check whether the on-disk binary is newer than `start_time`.
pub fn binary_changed(start_time: SystemTime) -> bool {
    exe_mtime().is_some_and(|mtime| mtime > start_time)
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

    #[test]
    fn binary_changed_true_when_start_time_is_epoch() {
        assert!(binary_changed(SystemTime::UNIX_EPOCH));
    }

    #[test]
    fn binary_changed_false_when_start_time_is_far_future() {
        let future = SystemTime::now() + Duration::from_secs(86400 * 365 * 10);
        assert!(!binary_changed(future));
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
