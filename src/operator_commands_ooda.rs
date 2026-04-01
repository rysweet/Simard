use std::path::PathBuf;

use crate::bridge_launcher::{
    find_python_dir, launch_gym_bridge, launch_knowledge_bridge, launch_memory_bridge,
};
use crate::goal_curation::load_goal_board;
use crate::ooda_loop::{
    OodaBridges, OodaConfig, OodaState, run_ooda_cycle, summarize_cycle_report,
};

/// Run one or more OODA cycles as a daemon-style loop.
///
/// Launches all bridges, loads the goal board from memory, and runs
/// OODA cycles until `max_cycles` is reached (0 = infinite).
pub fn run_ooda_daemon(
    max_cycles: u32,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = state_root_override.unwrap_or_else(|| {
        PathBuf::from(
            std::env::var("SIMARD_STATE_ROOT").unwrap_or_else(|_| "/tmp/simard-ooda".to_string()),
        )
    });

    std::fs::create_dir_all(&state_root)?;

    let agent_name =
        std::env::var("SIMARD_AGENT_NAME").unwrap_or_else(|_| "simard-ooda".to_string());

    let python_dir = find_python_dir()?;
    let db_path = state_root.join("cognitive_memory");

    let memory = launch_memory_bridge(&agent_name, &db_path, &python_dir)?;
    let knowledge = launch_knowledge_bridge(&python_dir)?;
    let gym = launch_gym_bridge(&python_dir)?;

    let bridges = OodaBridges {
        memory,
        knowledge,
        gym,
    };

    let board = load_goal_board(&bridges.memory).unwrap_or_default();
    let mut state = OodaState::new(board);
    let config = OodaConfig::default();

    let mut cycles_run = 0u32;

    loop {
        if max_cycles > 0 && cycles_run >= max_cycles {
            eprintln!("[simard] OODA daemon: completed {cycles_run} cycle(s), exiting");
            break;
        }

        match run_ooda_cycle(&mut state, &bridges, &config) {
            Ok(report) => {
                eprintln!("[simard] {}", summarize_cycle_report(&report));
            }
            Err(e) => {
                eprintln!("[simard] OODA cycle error: {e}");
            }
        }

        cycles_run += 1;

        // Sleep between cycles to avoid busy-looping. In production this
        // would be configurable; default is 60 seconds.
        std::thread::sleep(std::time::Duration::from_secs(60));
    }

    Ok(())
}
