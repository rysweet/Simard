use std::path::PathBuf;
use std::sync::Arc;

use simard::{
    BootstrapConfig, BootstrapInputs, BuiltinIdentityLoader, CoordinatedSupervisor,
    FilePromptAssetStore, IdentityLoadRequest, IdentityLoader, InMemoryEvidenceStore,
    InMemoryMemoryStore, LoopbackMailboxTransport, LoopbackMeshTopologyDriver, ManifestContract,
    Provenance, ReflectiveRuntime, RuntimePorts, RuntimeRequest, RuntimeTopology,
    UuidSessionIdGenerator, bootstrap_entrypoint, builtin_base_type_registry_for_manifest,
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
        other => return Err(format!("unsupported probe command '{other}'").into()),
    }

    Ok(())
}

fn prompt_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")
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
    let topology = parse_topology(topology)?;
    let contract = ManifestContract::new(
        bootstrap_entrypoint(),
        "bootstrap-config -> identity-loader -> runtime-ports -> local-runtime",
        vec![
            "probe:handoff-roundtrip".to_string(),
            format!("identity:{identity}"),
            format!("base-type:{base_type}"),
            format!("topology:{topology}"),
        ],
        Provenance::new("probe", "simard_operator_probe"),
        simard::Freshness::now()?,
    )?;
    let manifest = BuiltinIdentityLoader.load(&IdentityLoadRequest::new(
        identity.to_string(),
        env!("CARGO_PKG_VERSION"),
        contract,
    ))?;
    let request = RuntimeRequest::new(
        manifest.clone(),
        simard::BaseTypeId::new(base_type),
        topology,
    );

    let mut runtime = simard::LocalRuntime::compose(
        assemble_loopback_ports(
            Arc::new(FilePromptAssetStore::new(prompt_root())),
            builtin_base_type_registry_for_manifest(&manifest)?,
        )?,
        request.clone(),
    )?;
    runtime.start()?;
    let outcome = runtime.run(objective.to_string())?;
    let exported = runtime.export_handoff()?;

    let restored = simard::LocalRuntime::compose_from_handoff(
        assemble_loopback_ports(
            Arc::new(FilePromptAssetStore::new(prompt_root())),
            builtin_base_type_registry_for_manifest(&manifest)?,
        )?,
        request,
        exported.clone(),
    )?;
    let restored_snapshot = restored.snapshot()?;

    println!("Probe mode: handoff-roundtrip");
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
    println!("Execution summary: {}", outcome.execution_summary);
    Ok(())
}

fn assemble_loopback_ports(
    prompt_store: Arc<FilePromptAssetStore>,
    base_types: simard::BaseTypeRegistry,
) -> Result<RuntimePorts, Box<dyn std::error::Error>> {
    Ok(RuntimePorts::with_runtime_services(
        prompt_store,
        Arc::new(InMemoryMemoryStore::try_default()?),
        Arc::new(InMemoryEvidenceStore::try_default()?),
        base_types,
        Arc::new(LoopbackMeshTopologyDriver::try_default()?),
        Arc::new(LoopbackMailboxTransport::try_default()?),
        Arc::new(CoordinatedSupervisor::try_default()?),
        Arc::new(UuidSessionIdGenerator),
    ))
}

fn parse_topology(value: &str) -> Result<RuntimeTopology, Box<dyn std::error::Error>> {
    match value {
        "single-process" => Ok(RuntimeTopology::SingleProcess),
        "multi-process" => Ok(RuntimeTopology::MultiProcess),
        "distributed" => Ok(RuntimeTopology::Distributed),
        other => Err(format!("unsupported topology '{other}'").into()),
    }
}
