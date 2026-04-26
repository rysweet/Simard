use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

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
