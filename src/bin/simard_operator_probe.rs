use std::path::PathBuf;

use simard::{
    BootstrapConfig, BootstrapInputs, MemoryScope, ReflectiveRuntime,
    assemble_local_runtime_from_handoff, latest_local_handoff,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or("expected a probe command")?;

    match command.as_str() {
        "bootstrap-run" => {
            let identity = args.next().ok_or("expected identity")?;
            let base_type = args.next().ok_or("expected base type")?;
            let topology = args.next().ok_or("expected topology")?;
            let objective = args.next().ok_or("expected objective")?;
            run_bootstrap_probe(&identity, &base_type, &topology, &objective)?;
        }
        "handoff-roundtrip" => {
            let identity = args.next().ok_or("expected identity")?;
            let base_type = args.next().ok_or("expected base type")?;
            let topology = args.next().ok_or("expected topology")?;
            let objective = args.next().ok_or("expected objective")?;
            run_handoff_probe(&identity, &base_type, &topology, &objective)?;
        }
        "meeting-run" => {
            let base_type = args.next().ok_or("expected base type")?;
            let topology = args.next().ok_or("expected topology")?;
            let objective = args.next().ok_or("expected objective")?;
            run_meeting_probe(&base_type, &topology, &objective)?;
        }
        other => return Err(format!("unsupported probe command '{other}'").into()),
    }

    Ok(())
}

fn prompt_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")
}

fn state_root(identity: &str, base_type: &str, topology: &str, probe: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/operator-probe-state")
        .join(probe)
        .join(identity)
        .join(base_type)
        .join(topology)
}

fn run_bootstrap_probe(
    identity: &str,
    base_type: &str,
    topology: &str,
    objective: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(state_root(identity, base_type, topology, "bootstrap-run")),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = simard::run_local_session(&config)?;
    println!("Probe mode: bootstrap-run");
    println!("Identity: {}", execution.snapshot.identity_name);
    println!(
        "Identity components: {}",
        if execution.snapshot.identity_components.is_empty() {
            "<none>".to_string()
        } else {
            execution.snapshot.identity_components.join(", ")
        }
    );
    println!(
        "Selected base type: {}",
        execution.snapshot.selected_base_type
    );
    println!("Topology: {}", execution.snapshot.topology);
    println!(
        "Adapter implementation: {}",
        execution.snapshot.adapter_backend.identity
    );
    println!(
        "Topology backend: {}",
        execution.snapshot.topology_backend.identity
    );
    println!(
        "Transport backend: {}",
        execution.snapshot.transport_backend.identity
    );
    println!("State root: {}", config.state_root_path().display());
    println!("Session phase: {}", execution.outcome.session.phase);
    println!("Shutdown: {}", execution.stopped_snapshot.runtime_state);
    println!("Execution summary: {}", execution.outcome.execution_summary);
    println!(
        "Reflection summary: {}",
        execution.outcome.reflection.summary
    );
    Ok(())
}

fn run_handoff_probe(
    identity: &str,
    base_type: &str,
    topology: &str,
    objective: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(state_root(
            identity,
            base_type,
            topology,
            "handoff-roundtrip",
        )),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = simard::run_local_session(&config)?;
    let exported = latest_local_handoff(&config)?.ok_or("expected durable handoff snapshot")?;
    let restored = assemble_local_runtime_from_handoff(&config, exported.clone())?;
    let restored_snapshot = restored.snapshot()?;

    println!("Probe mode: handoff-roundtrip");
    println!("State root: {}", config.state_root_path().display());
    println!("Identity: {}", restored_snapshot.identity_name);
    println!(
        "Identity components: {}",
        if restored_snapshot.identity_components.is_empty() {
            "<none>".to_string()
        } else {
            restored_snapshot.identity_components.join(", ")
        }
    );
    println!(
        "Selected base type: {}",
        restored_snapshot.selected_base_type
    );
    println!("Topology: {}", restored_snapshot.topology);
    println!("Runtime node: {}", restored_snapshot.runtime_node);
    println!("Mailbox address: {}", restored_snapshot.mailbox_address);
    println!("Exported memory records: {}", exported.memory_records.len());
    println!(
        "Exported evidence records: {}",
        exported.evidence_records.len()
    );
    println!("Restored state: {}", restored_snapshot.runtime_state);
    println!(
        "Restored session phase: {}",
        restored_snapshot
            .session_phase
            .map(|phase| phase.to_string())
            .unwrap_or_else(|| "<none>".to_string())
    );
    println!(
        "Restored adapter implementation: {}",
        restored_snapshot.adapter_backend.identity
    );
    println!(
        "Restored topology backend: {}",
        restored_snapshot.topology_backend.identity
    );
    println!(
        "Restored transport backend: {}",
        restored_snapshot.transport_backend.identity
    );
    println!("Execution summary: {}", execution.outcome.execution_summary);
    Ok(())
}

fn run_meeting_probe(
    base_type: &str,
    topology: &str,
    objective: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let identity = "simard-meeting";
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(state_root(identity, base_type, topology, "meeting-run")),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = simard::run_local_session(&config)?;
    let exported = latest_local_handoff(&config)?.ok_or("expected durable meeting handoff")?;
    let decision_records = exported
        .memory_records
        .iter()
        .filter(|record| record.scope == MemoryScope::Decision)
        .map(|record| record.value.clone())
        .collect::<Vec<_>>();

    println!("Probe mode: meeting-run");
    println!("Identity: {}", execution.snapshot.identity_name);
    println!(
        "Selected base type: {}",
        execution.snapshot.selected_base_type
    );
    println!("Topology: {}", execution.snapshot.topology);
    println!("State root: {}", config.state_root_path().display());
    println!("Session phase: {}", execution.outcome.session.phase);
    println!("Decision records: {}", decision_records.len());
    for (index, value) in decision_records.iter().enumerate() {
        println!("Decision record {}: {}", index + 1, value);
    }
    println!("Execution summary: {}", execution.outcome.execution_summary);
    println!(
        "Reflection summary: {}",
        execution.outcome.reflection.summary
    );
    Ok(())
}
