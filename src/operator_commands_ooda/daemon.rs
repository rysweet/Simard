use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

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

    let mut cycles_run = 0u32;

    loop {
        // Check for shutdown signal at the top of each iteration.
        if shutdown.load(Ordering::SeqCst) {
            eprintln!("[simard] OODA daemon: shutting down gracefully");
            break;
        }

        if max_cycles > 0 && cycles_run >= max_cycles {
            eprintln!("[simard] OODA daemon: completed {cycles_run} cycle(s), exiting");
            break;
        }

        match run_ooda_cycle(&mut state, &mut bridges, &config) {
            Ok(report) => {
                let summary = summarize_cycle_report(&report);
                eprintln!("[simard] {summary}");
                // Persist the cycle report to filesystem for auditability.
                persist_cycle_report(&state_root, &report);
                // Persist the cycle summary to cognitive memory as an episode.
                persist_cycle_to_memory(&bridges, &report);
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
