use simard::{BootstrapConfig, run_local_session};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = BootstrapConfig::from_env()?;
    let execution = run_local_session(&config)?;

    println!("Simard local runtime executed successfully.");
    println!("Bootstrap mode: {}", config.mode);
    println!(
        "Config sources: prompt_root={}, objective={}, base_type={}, topology={}",
        config.prompt_root.source,
        config.objective.source,
        config.selected_base_type.source,
        config.topology.source
    );
    println!(
        "Bootstrap selection: identity={}, base_type={}, topology={}",
        config.identity, config.selected_base_type.value, config.topology.value
    );
    println!(
        "Snapshot: state={}, topology={}, base_type={}",
        execution.snapshot.runtime_state,
        execution.snapshot.topology,
        execution.snapshot.selected_base_type
    );
    println!("Shutdown: {}", execution.stopped_snapshot.runtime_state);

    Ok(())
}
