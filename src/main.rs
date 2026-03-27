use simard::{BootstrapConfig, ReflectiveRuntime, assemble_local_runtime};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = BootstrapConfig::from_env()?;
    let mut runtime = assemble_local_runtime(&config)?;
    runtime.start()?;

    let outcome = runtime.run(config.objective.value.clone())?;
    let snapshot = runtime.snapshot()?;
    runtime.stop()?;
    let stopped_snapshot = runtime.snapshot()?;

    println!("Simard local runtime executed successfully.");
    println!("Bootstrap mode: {}", config.mode);
    println!(
        "Config sources: prompt_root={}, objective={}",
        config.prompt_root.source, config.objective.source
    );
    println!("Plan: {}", outcome.plan);
    println!("Execution: {}", outcome.execution_summary);
    println!("Reflection: {}", outcome.reflection.summary);
    println!(
        "Snapshot: state={}, topology={}, base_type={}",
        snapshot.runtime_state, snapshot.topology, snapshot.selected_base_type
    );
    println!("Shutdown: {}", stopped_snapshot.runtime_state);

    Ok(())
}
