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
use crate::runtime_config::RuntimeConfig;
use crate::session_builder::{LlmProvider, SessionBuilder};

use crate::operator_commands_ooda::persistence::{persist_cycle_report, persist_cycle_to_memory};

mod helpers;
pub use helpers::*;

mod config;
pub use config::DaemonDashboardConfig;

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

    // One-time bootstrap: snapshot SIMARD_LLM_PROVIDER (if set in env)
    // to <state_root>/config.toml so child processes (engineer subprocesses
    // spawned via tmux, meeting REPLs, etc.) read the same configuration
    // without env-var propagation through every wrapper.
    match RuntimeConfig::bootstrap_from_env(&state_root) {
        Ok(true) => daemon_log(
            &state_root,
            "[simard] OODA daemon: wrote ~/.simard/config.toml from environment",
        ),
        Ok(false) => {}
        Err(e) => daemon_log(
            &state_root,
            &format!("[simard] OODA daemon: config bootstrap failed: {e}"),
        ),
    }

    // Open an LLM session for autonomous work. Required — no silent degradation.
    let provider = LlmProvider::resolve()
        .map_err(|e| format!("OODA daemon: LLM provider not configured: {e}"))?;
    let session = SessionBuilder::new(OperatingMode::Orchestrator, provider)
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

    // Issue #1197: sweep orphaned engineer worktrees from prior crashed
    // daemons before starting the loop, so disk pressure doesn't accumulate.
    if let Ok(parent_repo) = std::env::current_dir() {
        match crate::engineer_worktree::sweep_orphaned_worktrees(&parent_repo, &state_root) {
            Ok(report) => {
                if !report.removed_orphan_dirs.is_empty() {
                    daemon_log(
                        &state_root,
                        &format!(
                            "[simard] OODA daemon: swept {} orphan engineer worktree(s)",
                            report.removed_orphan_dirs.len()
                        ),
                    );
                }
            }
            Err(e) => daemon_log(
                &state_root,
                &format!("[simard] OODA daemon: engineer worktree sweep failed: {e}"),
            ),
        }
    }

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
        // Reap any zombie engineer subprocesses from the previous cycle's
        // spawns before doing anything else. Non-blocking; logs only when
        // a positive count was reaped to keep steady-state logs clean.
        let reaped = crate::agent_supervisor::reap_zombies();
        if reaped > 0 {
            daemon_log(
                &state_root,
                &format!("[simard] reaped {reaped} zombie engineer process(es)"),
            );
        }

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
