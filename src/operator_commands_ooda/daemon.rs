use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crate::bridge_launcher::{find_python_dir, launch_gym_bridge, launch_knowledge_bridge};
use crate::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use crate::goal_curation::load_goal_board;
use crate::identity::OperatingMode;
use crate::memory_ipc;
use crate::ooda_loop::{
    OodaBridges, OodaConfig, OodaPhase, OodaState, run_ooda_cycle, summarize_cycle_report,
};
use crate::session_builder::SessionBuilder;

use super::persistence::{persist_cycle_report, persist_cycle_to_memory};

/// Append a timestamped log line to `{state_root}/ooda.log` **and** stderr.
///
/// The dashboard `/api/logs` endpoint already looks for `ooda.log` inside the
/// state root, so writing here makes daemon output visible in the Logs tab
/// without requiring systemd or manual redirection.  Failures to write are
/// silently ignored — stderr is the primary output channel.
fn daemon_log(state_root: &std::path::Path, msg: &str) {
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

/// Configuration for the embedded dashboard that runs inside the OODA daemon.
pub struct DaemonDashboardConfig {
    /// Whether to spawn the dashboard as a background task.
    pub enabled: bool,
    /// TCP port for the dashboard (default: 8080, overridable via
    /// `SIMARD_DASHBOARD_PORT` env var or `--dashboard-port=` CLI flag).
    pub port: u16,
}

impl Default for DaemonDashboardConfig {
    fn default() -> Self {
        let port = std::env::var("SIMARD_DASHBOARD_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8080);
        Self {
            enabled: true,
            port,
        }
    }
}

/// Run one or more OODA cycles as a daemon-style loop.
///
/// Launches all bridges, opens a RustyClawd session via [`SessionBuilder`]
/// for real autonomous work, loads the goal board from cognitive memory,
/// and runs OODA cycles until `max_cycles` is reached (0 = infinite).
///
/// When `dashboard.enabled` is true, the dashboard's axum server is spawned
/// as a background tokio task — sharing the same process and restarting
/// automatically when the daemon restarts (via auto-reload or systemd).
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
    dashboard: DaemonDashboardConfig,
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

    let state_root = state_root_override.unwrap_or_else(memory_ipc::default_state_root);

    std::fs::create_dir_all(&state_root)?;

    let python_dir = find_python_dir()?;

    // Reap any stale lock file from a prior crashed daemon before we open.
    if let Err(e) = memory_ipc::reap_stale_open_lock(&state_root) {
        eprintln!("[simard] OODA daemon: stale-lock reap failed: {e}");
    }

    let shared_mem: Arc<dyn CognitiveMemoryOps> =
        Arc::new(NativeCognitiveMemory::open(&state_root)?);

    // Spawn the memory IPC server so meetings and other clients can share
    // this live DB handle without their own locks conflicting.
    let socket_path = memory_ipc::default_socket_path();
    let _memory_ipc_server = match memory_ipc::spawn_server(socket_path.clone(), shared_mem.clone())
    {
        Ok(h) => {
            daemon_log(
                &state_root,
                &format!(
                    "[simard] OODA daemon: memory IPC listening at {}",
                    socket_path.display()
                ),
            );
            Some(h)
        }
        Err(e) => {
            daemon_log(
                &state_root,
                &format!(
                    "[simard] OODA daemon: memory IPC server failed to start: {e} \
                     (meetings will fall back to direct open)"
                ),
            );
            None
        }
    };

    let memory: Box<dyn CognitiveMemoryOps> = Box::new(memory_ipc::SharedMemory(shared_mem));
    let knowledge = launch_knowledge_bridge(&python_dir)?;
    let gym = launch_gym_bridge(&python_dir)?;

    // Open an LLM session for autonomous work. Required — no silent degradation.
    let session = SessionBuilder::new(OperatingMode::Orchestrator)
        .node_id("ooda-daemon")
        .address("ooda-daemon://local")
        .adapter_tag("ooda")
        .open()
        .map_err(|e| format!("OODA daemon requires LLM session but open() failed: {e}"))?;
    daemon_log(
        &state_root,
        "[simard] OODA daemon: LLM session opened for autonomous work",
    );

    let mut bridges = OodaBridges {
        memory,
        knowledge,
        gym,
        session: Some(session),
    };

    let board = load_goal_board(&*bridges.memory).unwrap_or_default();
    let mut state = OodaState::new(board);
    let config = OodaConfig::default();

    let interval_secs: u64 = std::env::var("SIMARD_OODA_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);

    daemon_log(
        &state_root,
        &format!("[simard] OODA daemon: cycle interval = {interval_secs}s"),
    );

    // --- embedded dashboard ------------------------------------------------
    // Spawn the dashboard as a background tokio task so both OODA loop and
    // dashboard share a single process. On daemon restart (auto-reload or
    // systemd), the dashboard restarts automatically.
    let _dashboard_rt;
    let _dashboard_handle;
    if dashboard.enabled {
        let (code, loaded) = crate::operator_commands_dashboard::init_auth();
        eprintln!("\n  🌲 Simard Dashboard (embedded in OODA daemon)");
        if loaded {
            eprintln!("  Login code: {code} (loaded from ~/.simard/.dashkey)");
        } else {
            eprintln!("  Login code: {code} (saved to ~/.simard/.dashkey)");
        }
        eprintln!(
            "  Open http://localhost:{} and enter the code\n",
            dashboard.port
        );

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let handle =
            crate::operator_commands_dashboard::spawn_dashboard_task(rt.handle(), dashboard.port);
        _dashboard_rt = Some(rt);
        _dashboard_handle = Some(handle);
    } else {
        _dashboard_rt = None;
        _dashboard_handle = None;
        daemon_log(
            &state_root,
            "[simard] OODA daemon: dashboard disabled (use --no-dashboard to suppress)",
        );
    }
    // -----------------------------------------------------------------------

    // Capture the binary mtime at startup so we can detect in-place upgrades.
    let start_time = exe_mtime().unwrap_or_else(SystemTime::now);

    if auto_reload {
        daemon_log(&state_root, "[simard] OODA daemon: auto-reload enabled");
    }

    // --- periodic DB backup state -----------------------------------------
    let db_backup_interval_secs: u64 = std::env::var("SIMARD_DB_BACKUP_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3600);
    let mut last_db_backup = Instant::now()
        .checked_sub(Duration::from_secs(db_backup_interval_secs))
        .unwrap_or_else(Instant::now);
    daemon_log(
        &state_root,
        &format!("[simard] OODA daemon: DB backup interval = {db_backup_interval_secs}s"),
    );
    // -------------------------------------------------------------------

    let mut cycles_run = 0u32;

    loop {
        // Check for shutdown signal at the top of each iteration.
        if shutdown.load(Ordering::SeqCst) {
            daemon_log(
                &state_root,
                "[simard] OODA daemon: shutting down gracefully",
            );
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
            daemon_log(
                &state_root,
                &format!("[simard] OODA daemon: completed {cycles_run} cycle(s), exiting"),
            );
            break;
        }

        // --- periodic DB backup (at START of cycle) ------------------------
        if last_db_backup.elapsed() >= Duration::from_secs(db_backup_interval_secs) {
            match NativeCognitiveMemory::create_verified_backup(&state_root) {
                Ok(backup_path) => {
                    daemon_log(
                        &state_root,
                        &format!("[simard] DB backup created: {}", backup_path.display()),
                    );
                    NativeCognitiveMemory::prune_old_backups(&state_root, 5);
                }
                Err(e) => {
                    daemon_log(&state_root, &format!("[simard] DB backup failed: {e}"));
                }
            }
            last_db_backup = Instant::now();
        }
        // -------------------------------------------------------------------

        let cycle_start = Instant::now();
        let cycle_start_epoch = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        state.cycle_start_epoch = cycle_start_epoch;

        // Write heartbeat at cycle START so the dashboard never sees "stale"
        // during a long-running cycle.
        {
            let health_dir = dirs::data_local_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"))
                .join("simard");
            let _ = std::fs::create_dir_all(&health_dir);
            let heartbeat = serde_json::json!({
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "cycle_number": cycles_run + 1,
                "status": "running",
                "cycle_phase": state.current_phase.to_string(),
                "cycle_start_epoch": cycle_start_epoch,
                "interval_secs": interval_secs,
                "actions_taken": format!("Starting cycle #{}", cycles_run + 1),
            });
            let _ = std::fs::write(
                health_dir.join("daemon_health.json"),
                serde_json::to_string_pretty(&heartbeat).unwrap_or_default(),
            );
        }

        match run_ooda_cycle(&mut state, &mut bridges, &config) {
            Ok(report) => {
                let cycle_elapsed = cycle_start.elapsed();
                let summary = summarize_cycle_report(&report);
                state.last_cycle_summary = Some(summary.clone());
                state.last_cycle_duration_secs = Some(cycle_elapsed.as_secs());
                state.current_phase = OodaPhase::Sleep;
                daemon_log(&state_root, &format!("[simard] {summary}"));
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
                        "cycle_phase": "sleep",
                        "cycle_start_epoch": cycle_start_epoch,
                        "cycle_duration_secs": cycle_elapsed.as_secs(),
                        "interval_secs": interval_secs,
                        "actions_taken": summary.clone(),
                        "last_cycle_summary": summary,
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
                daemon_log(&state_root, &format!("[simard] OODA cycle error: {e}"));
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

    #[test]
    fn daemon_dashboard_config_default_values() {
        // Clear any env override to test the true default
        unsafe { std::env::remove_var("SIMARD_DASHBOARD_PORT") };
        let config = DaemonDashboardConfig::default();
        assert!(config.enabled);
        assert_eq!(config.port, 8080);
    }

    #[test]
    fn daemon_dashboard_config_env_override() {
        unsafe { std::env::set_var("SIMARD_DASHBOARD_PORT", "9090") };
        let config = DaemonDashboardConfig::default();
        assert_eq!(config.port, 9090);
        // Clean up
        unsafe { std::env::remove_var("SIMARD_DASHBOARD_PORT") };
    }

    #[test]
    fn daemon_dashboard_config_invalid_env_falls_back() {
        unsafe { std::env::set_var("SIMARD_DASHBOARD_PORT", "not_a_number") };
        let config = DaemonDashboardConfig::default();
        assert_eq!(config.port, 8080);
        unsafe { std::env::remove_var("SIMARD_DASHBOARD_PORT") };
    }

    #[test]
    fn daemon_log_writes_to_stderr_and_file() {
        let dir = tempfile::tempdir().unwrap();
        daemon_log(dir.path(), "test daemon log message");
        let log_path = dir.path().join("ooda.log");
        assert!(log_path.is_file());
        let contents = std::fs::read_to_string(&log_path).unwrap();
        assert!(contents.contains("test daemon log message"));
    }

    #[test]
    fn daemon_log_appends_not_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        daemon_log(dir.path(), "first message");
        daemon_log(dir.path(), "second message");
        let contents = std::fs::read_to_string(dir.path().join("ooda.log")).unwrap();
        assert!(contents.contains("first message"));
        assert!(contents.contains("second message"));
    }

    #[test]
    fn exe_mtime_is_in_reasonable_range() {
        let mtime = exe_mtime().unwrap();
        let elapsed = mtime.elapsed().unwrap_or(Duration::ZERO);
        // The test binary should have been built recently (within last year)
        assert!(elapsed < Duration::from_secs(365 * 24 * 3600));
    }

    #[test]
    fn daemon_log_creates_file_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("ooda.log");
        assert!(!log_path.exists());
        daemon_log(dir.path(), "creation test");
        assert!(log_path.exists());
    }

    #[test]
    fn daemon_log_includes_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        daemon_log(dir.path(), "timestamped message");
        let contents = std::fs::read_to_string(dir.path().join("ooda.log")).unwrap();
        // Timestamp format: YYYY-MM-DDTHH:MM:SSZ
        assert!(
            contents.contains('T') && contents.contains('Z'),
            "log should contain ISO timestamp, got: {contents}"
        );
    }

    #[test]
    fn binary_changed_false_for_current_time() {
        // If start_time is now, binary should not appear changed.
        assert!(!binary_changed(SystemTime::now()));
    }

    #[test]
    fn interruptible_sleep_very_short_duration() {
        let shutdown = AtomicBool::new(false);
        let start = Instant::now();
        interruptible_sleep(Duration::from_millis(1), &shutdown);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn dashboard_config_fields_are_independent() {
        let config = DaemonDashboardConfig {
            enabled: false,
            port: 3000,
        };
        assert!(!config.enabled);
        assert_eq!(config.port, 3000);
    }
}
