use simard::{BootstrapConfig, run_local_session};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = BootstrapConfig::from_env()?;
    let execution = run_local_session(&config)?;

    println!("Simard local runtime executed successfully.");
    println!("Bootstrap mode: {}", config.mode);
    println!(
        "Config sources: prompt_root={}, objective={}",
        config.prompt_root.source, config.objective.source
    );
    println!("Plan: {}", execution.outcome.plan);
    println!("Execution: {}", execution.outcome.execution_summary);
    println!("Reflection: {}", execution.outcome.reflection.summary);
    println!(
        "Snapshot: state={}, topology={}, base_type={}",
        execution.snapshot.runtime_state,
        execution.snapshot.topology,
        execution.snapshot.selected_base_type
    );
    println!("Shutdown: {}", execution.stopped_snapshot.runtime_state);

    Ok(())
}
