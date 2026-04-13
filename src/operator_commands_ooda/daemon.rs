use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crate::bridge_launcher::{
    cognitive_memory_db_path, find_python_dir, launch_gym_bridge, launch_knowledge_bridge,
    launch_memory_bridge,
};
use crate::goal_curation::load_goal_board;
use crate::identity::OperatingMode;
use crate::ooda_loop::{
    OodaBridges, OodaConfig, OodaState, run_ooda_cycle, summarize_cycle_report,
};
use crate::session_builder::SessionBuilder;

use super::persistence::{persist_cycle_report, persist_cycle_to_memory};

/// Return the mtime of the currently-running executable, or `None` if it
/// cannot be determined (e.g. the binary was deleted after launch).
fn exe_mtime() -> Option<SystemTime> {
    std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
}

/// Check whether the on-disk binary is newer than `start_time`.
pub(crate) fn binary_changed(start_time: SystemTime) -> bool {
    exe_mtime().is_some_and(|mtime| mtime > start_time)
}

/// Replace the current process with a fresh copy of itself.
///
/// On success this function never returns — the process image is replaced
/// via `exec()`.  On failure the error is returned so the caller can
/// degrade gracefully and continue running.
#[cfg(unix)]
fn exec_self_reload() -> Result<(), Box<dyn std::error::Error>> {
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
fn interruptible_sleep(total: Duration, shutdown: &AtomicBool) {
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

/// Run one or more OODA cycles as a daemon-style loop.
///
/// Launches all bridges, opens a RustyClawd session via [`SessionBuilder`]
/// for real autonomous work, loads the goal board from cognitive memory,
/// and runs OODA cycles until `max_cycles` is reached (0 = infinite).
///
/// On SIGTERM/SIGINT the current cycle finishes, the session is closed
/// cleanly, and the daemon exits without orphaning PTY subprocesses.
///
/// If no LLM adapter is available (e.g. no API key, no Copilot SDK),
/// the daemon exits with an error — no silent degradation to bridge-only mode.
pub fn run_ooda_daemon(
    max_cycles: u32,
    state_root_override: Option<PathBuf>,
    auto_reload: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // --- signal handling ------------------------------------------------
    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let flag = Arc::clone(&shutdown);
        ctrlc::set_handler(move || {
            flag.store(true, Ordering::SeqCst);
        })
        .expect("failed to install SIGTERM/SIGINT handler");
    }
    // --------------------------------------------------------------------

    // Auto-ensure runtime dependencies before launching bridges
    if let Err(e) = crate::cmd_ensure_deps::handle_ensure_deps() {
        eprintln!("Warning: some dependencies could not be verified: {e}");
    }

    let state_root = state_root_override.unwrap_or_else(|| {
        PathBuf::from(
            std::env::var("SIMARD_STATE_ROOT").unwrap_or_else(|_| "/tmp/simard-ooda".to_string()),
        )
    });

    std::fs::create_dir_all(&state_root)?;

    let agent_name =
        std::env::var("SIMARD_AGENT_NAME").unwrap_or_else(|_| "simard-ooda".to_string());

    let python_dir = find_python_dir()?;
    let db_path = cognitive_memory_db_path(&state_root);

    let memory = launch_memory_bridge(&agent_name, &db_path, &python_dir)?;
    let knowledge = launch_knowledge_bridge(&python_dir)?;
    let gym = launch_gym_bridge(&python_dir)?;

    // Open an LLM session for autonomous work. Required — no silent degradation.
    let session = SessionBuilder::new(OperatingMode::Orchestrator)
        .node_id("ooda-daemon")
        .address("ooda-daemon://local")
        .adapter_tag("ooda")
        .open()
        .map_err(|e| format!("OODA daemon requires LLM session but open() failed: {e}"))?;
    eprintln!("[simard] OODA daemon: LLM session opened for autonomous work");

    let mut bridges = OodaBridges {
        memory,
        knowledge,
        gym,
        session: Some(session),
    };

    let board = load_goal_board(&bridges.memory).unwrap_or_default();
    let mut state = OodaState::new(board);
    let config = OodaConfig::default();

    let interval_secs: u64 = std::env::var("SIMARD_OODA_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);

    eprintln!("[simard] OODA daemon: cycle interval = {interval_secs}s");

    // Capture the binary mtime at startup so we can detect in-place upgrades.
    let start_time = exe_mtime().unwrap_or_else(SystemTime::now);

    if auto_reload {
        eprintln!("[simard] OODA daemon: auto-reload enabled");
    }

    let mut cycles_run = 0u32;

    loop {
        // Check for shutdown signal at the top of each iteration.
        if shutdown.load(Ordering::SeqCst) {
            eprintln!("[simard] OODA daemon: shutting down gracefully");
            break;
        }

        // Auto-reload: if the on-disk binary is newer, exec into it.
        #[cfg(unix)]
        if auto_reload && binary_changed(start_time) {
            // Close the LLM session before exec so we don't leak resources.
            if let Some(ref mut session) = bridges.session {
                let _ = session.close();
            }
            exec_self_reload()?;
            // exec_self_reload only returns on error — continue running.
        }

        if max_cycles > 0 && cycles_run >= max_cycles {
            eprintln!("[simard] OODA daemon: completed {cycles_run} cycle(s), exiting");
            break;
        }

        let cycle_start = Instant::now();

        match run_ooda_cycle(&mut state, &mut bridges, &config) {
            Ok(report) => {
                let cycle_elapsed = cycle_start.elapsed();
                let summary = summarize_cycle_report(&report);
                eprintln!("[simard] {summary}");
                // Persist the cycle report to filesystem for auditability.
                persist_cycle_report(&state_root, &report);
                // Persist the cycle summary to cognitive memory as an episode.
                persist_cycle_to_memory(&bridges, &report);
                // Write daemon health file for dashboard
                {
                    let health_dir = dirs::data_local_dir()
                        .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"))
                        .join("simard");
                    let _ = std::fs::create_dir_all(&health_dir);
                    let health = serde_json::json!({
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                        "cycle_number": cycles_run + 1,
                        "status": "healthy",
                        "actions_taken": summary.clone(),
                    });
                    let health_path = health_dir.join("daemon_health.json");
                    if let Err(e) = std::fs::write(
                        &health_path,
                        serde_json::to_string_pretty(&health).unwrap_or_default(),
                    ) {
                        eprintln!("[simard] OODA health: failed to write health file: {e}");
                    }
                }
                // Collect self-improvement metrics at end of each cycle.
                if let Err(e) = crate::self_metrics::collect_and_record_all(cycle_elapsed) {
                    eprintln!("[simard] OODA metrics: failed to record: {e}");
                }
            }
            Err(e) => {
                eprintln!("[simard] OODA cycle error: {e}");
            }
        }

        cycles_run += 1;

        // Skip the inter-cycle sleep if this was the last requested cycle.
        if max_cycles > 0 && cycles_run >= max_cycles {
            continue;
        }

        // Interruptible sleep — wakes early on SIGTERM/SIGINT instead of
        // blocking for the full interval.
        interruptible_sleep(Duration::from_secs(interval_secs), &shutdown);
    }

    // Close the session cleanly if it was opened.
    if let Some(ref mut session) = bridges.session {
        let _ = session.close();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interruptible_sleep_returns_immediately_on_shutdown() {
        let shutdown = AtomicBool::new(true);
        let start = Instant::now();
        interruptible_sleep(Duration::from_secs(60), &shutdown);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn test_interruptible_sleep_completes_short_duration() {
        let shutdown = AtomicBool::new(false);
        let start = Instant::now();
        interruptible_sleep(Duration::from_millis(100), &shutdown);
        assert!(start.elapsed() >= Duration::from_millis(100));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn test_interruptible_sleep_zero_duration() {
        let shutdown = AtomicBool::new(false);
        let start = Instant::now();
        interruptible_sleep(Duration::ZERO, &shutdown);
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn test_binary_changed_false_for_future_time() {
        // If start_time is far in the future, binary should not appear changed.
        let future = SystemTime::now() + Duration::from_secs(86400 * 365 * 10);
        assert!(!binary_changed(future));
    }

    #[test]
    fn test_exe_mtime_returns_some() {
        // The test binary itself should have a valid mtime.
        let mtime = exe_mtime();
        assert!(mtime.is_some());
    }

    #[test]
    fn test_binary_changed_true_for_epoch() {
        // If start_time is UNIX_EPOCH, the binary is certainly newer.
        let epoch = SystemTime::UNIX_EPOCH;
        assert!(binary_changed(epoch));
    }

    #[test]
    fn test_interruptible_sleep_mid_shutdown() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&shutdown);
        // Set shutdown after 100ms
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            flag.store(true, Ordering::SeqCst);
        });
        let start = Instant::now();
        interruptible_sleep(Duration::from_secs(60), &shutdown);
        // Should return well before 60s
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
