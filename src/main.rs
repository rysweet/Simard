use simard::{BootstrapConfig, dispatch_operator_cli, run_local_session};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if !args.is_empty() {
        return dispatch_operator_cli(args);
    }

    run_legacy_bootstrap_entrypoint()
}

fn run_legacy_bootstrap_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
    let config = BootstrapConfig::from_env()?;
    let execution = run_local_session(&config)?;

    println!("Simard local runtime executed successfully.");
    println!("Bootstrap mode: {}", config.mode);
    println!(
        "Config sources: prompt_root={}, objective={}, state_root={}, base_type={}, topology={}",
        config.prompt_root.source,
        config.objective.source,
        config.state_root.source,
        config.selected_base_type.source,
        config.topology.source
    );
    println!(
        "Bootstrap selection: identity={}, base_type={}, topology={}",
        config.identity, config.selected_base_type.value, config.topology.value
    );
    println!("State root: {}", config.state_root.value.display());
    if !execution.snapshot.identity_components.is_empty() {
        println!(
            "Identity components: {}",
            execution.snapshot.identity_components.join(", ")
        );
    }
    println!(
        "Snapshot: state={}, topology={}, base_type={}",
        execution.snapshot.runtime_state,
        execution.snapshot.topology,
        execution.snapshot.selected_base_type
    );
    println!(
        "Adapter implementation: {}",
        execution.snapshot.adapter_backend.identity
    );
    println!("Shutdown: {}", execution.stopped_snapshot.runtime_state);

    Ok(())
}
