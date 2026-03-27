use std::sync::Arc;

use simard::{
    BaseTypeId, BaseTypeRegistry, BootstrapConfig, BuiltinIdentityLoader, FilePromptAssetStore,
    IdentityLoadRequest, IdentityLoader, InMemoryEvidenceStore, InMemoryMemoryStore,
    LocalProcessHarnessAdapter, LocalRuntime, ReflectiveRuntime, RuntimePorts, RuntimeRequest,
    RuntimeTopology,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = BootstrapConfig::from_env()?;
    let prompt_store = Arc::new(FilePromptAssetStore::new(config.prompt_root.value.clone()));
    let memory_store = Arc::new(InMemoryMemoryStore::default());
    let evidence_store = Arc::new(InMemoryEvidenceStore::default());

    let mut base_types = BaseTypeRegistry::default();
    base_types.register(LocalProcessHarnessAdapter::single_process("local-harness"));

    let manifest = BuiltinIdentityLoader.load(&IdentityLoadRequest::new(
        config.identity.clone(),
        env!("CARGO_PKG_VERSION"),
        config.manifest_precedence(),
    ))?;

    let request = RuntimeRequest::new(
        manifest,
        BaseTypeId::new("local-harness"),
        RuntimeTopology::SingleProcess,
    );

    let ports = RuntimePorts::new(prompt_store, memory_store, evidence_store, base_types);
    let mut runtime = LocalRuntime::compose(ports, request)?;
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
