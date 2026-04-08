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

/// Write a `daemon_health.json` file the dashboard can read instead of pgrep.
fn write_health_file(
    state_root: &std::path::Path,
    start_time: &Instant,
    cycles_run: u32,
    last_action: Option<&str>,
) {
    let uptime_secs = start_time.elapsed().as_secs();
    let rss_kb = read_rss_kb().unwrap_or(0);
    let health = serde_json::json!({
        "status": "running",
        "uptime_secs": uptime_secs,
        "cycle_count": cycles_run,
        "last_action_time": last_action.unwrap_or("none"),
        "memory_rss_kb": rss_kb,
        "pid": std::process::id(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    let path = state_root.join("daemon_health.json");
    let _ = std::fs::write(
        &path,
        serde_json::to_string_pretty(&health).unwrap_or_default(),
    );
}

/// Read resident set size from /proc/self/status (Linux only).
fn read_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return rest.trim().trim_end_matches(" kB").trim().parse().ok();
        }
    }
    None
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
/// If no `ANTHROPIC_API_KEY` is set, the session will be `None` and the
/// daemon degrades honestly to bridge-only dispatch (Pillar 11).
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

    // Try to open an LLM session for real autonomous work.
    // The provider is selected by SIMARD_LLM_PROVIDER (default: Copilot).
    let session = SessionBuilder::new(OperatingMode::Orchestrator)
        .node_id("ooda-daemon")
        .address("ooda-daemon://local")
        .adapter_tag("ooda")
        .open();

    if session.is_some() {
        eprintln!("[simard] OODA daemon: LLM session opened for autonomous work");
    } else {
        eprintln!("[simard] OODA daemon: no LLM session available — running in bridge-only mode");
    }

    let mut bridges = OodaBridges {
        memory,
        knowledge,
        gym,
        session,
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

    let daemon_start = Instant::now();
    let mut cycles_run = 0u32;
    let mut last_action_time: Option<String> = None;

    // Write initial health file so the dashboard can detect the daemon immediately.
    write_health_file(&state_root, &daemon_start, 0, None);

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

        match run_ooda_cycle(&mut state, &mut bridges, &config) {
            Ok(report) => {
                let summary = summarize_cycle_report(&report);
                eprintln!("[simard] {summary}");
                persist_cycle_report(&state_root, &report);
                persist_cycle_to_memory(&bridges, &report);
                last_action_time = Some(chrono::Utc::now().to_rfc3339());
            }
            Err(e) => {
                eprintln!("[simard] OODA cycle error: {e}");
            }
        }

        cycles_run += 1;

        // Update health file after each cycle.
        write_health_file(
            &state_root,
            &daemon_start,
            cycles_run,
            last_action_time.as_deref(),
        );

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
