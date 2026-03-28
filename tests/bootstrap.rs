use std::path::PathBuf;

use simard::{
    BaseTypeId, BootstrapConfig, BootstrapInputs, BootstrapMode, BuiltinIdentityLoader,
    ConfigValueSource, IdentityLoadRequest, IdentityLoader, ManifestContract, Provenance,
    ReflectiveRuntime, RuntimeState, RuntimeTopology, SimardError, assemble_local_runtime,
    bootstrap_entrypoint, run_local_session,
};

#[test]
fn bootstrap_requires_explicit_prompt_root_and_objective_by_default() {
    let error = BootstrapConfig::resolve(BootstrapInputs::default()).unwrap_err();

    assert_eq!(
        error,
        simard::SimardError::MissingRequiredConfig {
            key: "SIMARD_PROMPT_ROOT".to_string(),
            help: "set SIMARD_PROMPT_ROOT or opt in with SIMARD_BOOTSTRAP_MODE=builtin-defaults"
                .to_string(),
        }
    );
}

#[test]
fn bootstrap_builtin_defaults_are_only_used_with_explicit_opt_in() {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        mode: Some("builtin-defaults".to_string()),
        ..BootstrapInputs::default()
    })
    .expect("builtin defaults should be allowed when explicitly requested");

    assert_eq!(config.mode, BootstrapMode::BuiltinDefaults);
    assert_eq!(config.identity, "simard-engineer");
    assert_eq!(
        config.prompt_root.source,
        ConfigValueSource::ExplicitOptIn("SIMARD_BOOTSTRAP_MODE")
    );
    assert_eq!(
        config.objective.source,
        ConfigValueSource::ExplicitOptIn("SIMARD_BOOTSTRAP_MODE")
    );
    assert_eq!(
        config.selected_base_type.source,
        ConfigValueSource::ExplicitOptIn("SIMARD_BOOTSTRAP_MODE")
    );
    assert_eq!(
        config.topology.source,
        ConfigValueSource::ExplicitOptIn("SIMARD_BOOTSTRAP_MODE")
    );
    assert_eq!(
        config.prompt_root.value,
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")
    );
    assert_eq!(config.objective.value, "bootstrap the Simard engineer loop");
    assert_eq!(
        config.selected_base_type.value,
        BaseTypeId::new("local-harness")
    );
    assert_eq!(config.topology.value, RuntimeTopology::SingleProcess);
}

#[test]
fn bootstrap_requires_explicit_identity_without_builtin_defaults() {
    let error = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
        objective: Some("exercise bootstrap identity handling".to_string()),
        base_type: Some("local-harness".to_string()),
        topology: Some("single-process".to_string()),
        ..BootstrapInputs::default()
    })
    .unwrap_err();

    assert_eq!(
        error,
        simard::SimardError::MissingRequiredConfig {
            key: "SIMARD_IDENTITY".to_string(),
            help: "set SIMARD_IDENTITY or opt in with SIMARD_BOOTSTRAP_MODE=builtin-defaults"
                .to_string(),
        }
    );
}

#[test]
fn bootstrap_requires_explicit_base_type_without_builtin_defaults() {
    let error = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
        objective: Some("exercise bootstrap base type handling".to_string()),
        identity: Some("simard-engineer".to_string()),
        topology: Some("single-process".to_string()),
        ..BootstrapInputs::default()
    })
    .unwrap_err();

    assert_eq!(
        error,
        SimardError::MissingRequiredConfig {
            key: "SIMARD_BASE_TYPE".to_string(),
            help: "set SIMARD_BASE_TYPE or opt in with SIMARD_BOOTSTRAP_MODE=builtin-defaults"
                .to_string(),
        }
    );
}

#[test]
fn bootstrap_requires_explicit_topology_without_builtin_defaults() {
    let error = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
        objective: Some("exercise bootstrap topology handling".to_string()),
        identity: Some("simard-engineer".to_string()),
        base_type: Some("local-harness".to_string()),
        ..BootstrapInputs::default()
    })
    .unwrap_err();

    assert_eq!(
        error,
        SimardError::MissingRequiredConfig {
            key: "SIMARD_RUNTIME_TOPOLOGY".to_string(),
            help:
                "set SIMARD_RUNTIME_TOPOLOGY or opt in with SIMARD_BOOTSTRAP_MODE=builtin-defaults"
                    .to_string(),
        }
    );
}

#[test]
fn bootstrap_rejects_invalid_topology_values() {
    let error = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
        objective: Some("exercise invalid topology handling".to_string()),
        identity: Some("simard-engineer".to_string()),
        base_type: Some("local-harness".to_string()),
        topology: Some("mystery-mesh".to_string()),
        ..BootstrapInputs::default()
    })
    .unwrap_err();

    assert_eq!(
        error,
        SimardError::InvalidConfigValue {
            key: "SIMARD_RUNTIME_TOPOLOGY".to_string(),
            value: "mystery-mesh".to_string(),
            help: "expected 'single-process', 'multi-process', or 'distributed'".to_string(),
        }
    );
}

#[test]
fn builtin_identity_loader_preserves_manifest_contract_metadata() {
    let contract = ManifestContract::new(
        bootstrap_entrypoint(),
        "bootstrap-config -> identity-loader -> runtime-ports -> local-runtime",
        vec![
            "mode:explicit-config".to_string(),
            "prompt-root:env:SIMARD_PROMPT_ROOT".to_string(),
        ],
        Provenance::new("bootstrap", bootstrap_entrypoint()),
        simard::Freshness::now().expect("freshness should be observable"),
    )
    .expect("contract should be valid");

    let manifest = BuiltinIdentityLoader
        .load(&IdentityLoadRequest::new(
            "simard-engineer",
            "0.1.0",
            contract.clone(),
        ))
        .expect("builtin identity should load");

    assert_eq!(manifest.contract, contract);
}

#[test]
fn bootstrap_assembly_produces_truthful_manifest_metadata() {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
        objective: Some("exercise bootstrap assembly".to_string()),
        identity: Some("simard-engineer".to_string()),
        base_type: Some("local-harness".to_string()),
        topology: Some("single-process".to_string()),
        ..BootstrapInputs::default()
    })
    .expect("explicit bootstrap config should resolve");

    let runtime = assemble_local_runtime(&config).expect("assembly should succeed");
    let snapshot = runtime.snapshot().expect("snapshot should succeed");

    assert_eq!(snapshot.runtime_state, RuntimeState::Initializing);
    assert_eq!(
        snapshot.manifest_contract.entrypoint,
        bootstrap_entrypoint()
    );
    assert_eq!(
        snapshot.manifest_contract.precedence,
        config.manifest_precedence()
    );
    assert_eq!(snapshot.manifest_contract.provenance.source, "bootstrap");
    assert_eq!(
        snapshot.selected_base_type,
        BaseTypeId::new("local-harness")
    );
    assert_eq!(snapshot.topology, RuntimeTopology::SingleProcess);
    assert!(
        snapshot
            .manifest_contract
            .provenance
            .locator
            .contains(bootstrap_entrypoint()),
        "manifest provenance should identify the bootstrap assembly boundary"
    );
}

#[test]
fn main_is_thin_and_bootstrap_owns_identity_and_runtime_assembly() {
    let main_rs = include_str!("../src/main.rs");
    let bootstrap_rs = include_str!("../src/bootstrap.rs");

    for forbidden in [
        "BuiltinIdentityLoader",
        "IdentityLoadRequest",
        "RuntimeRequest::new",
        "LocalRuntime::compose",
        "assemble_local_runtime",
        ".start()",
        ".run(",
        ".stop()",
    ] {
        assert!(
            !main_rs.contains(forbidden),
            "main.rs should stay as a thin executable root and not own {forbidden}"
        );
    }

    for required in [
        "BuiltinIdentityLoader",
        "IdentityLoadRequest",
        "RuntimeRequest::new",
        "LocalRuntime::compose",
        "run_local_session",
    ] {
        assert!(
            bootstrap_rs.contains(required),
            "bootstrap.rs should own {required} after identity/runtime extraction"
        );
    }
}

#[test]
fn main_does_not_print_objective_derived_runtime_details() {
    let main_rs = include_str!("../src/main.rs");

    for forbidden in [
        "println!(\"Plan:",
        "println!(\"Execution:",
        "println!(\"Reflection:",
        "execution.outcome.plan",
        "execution.outcome.execution_summary",
        "execution.outcome.reflection.summary",
    ] {
        assert!(
            !main_rs.contains(forbidden),
            "main.rs should not print objective-derived runtime details like {forbidden}"
        );
    }
}

#[test]
fn bootstrap_run_local_session_executes_the_cli_lifecycle() {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
        objective: Some("exercise the bootstrap run loop".to_string()),
        identity: Some("simard-engineer".to_string()),
        base_type: Some("local-harness".to_string()),
        topology: Some("single-process".to_string()),
        ..BootstrapInputs::default()
    })
    .expect("explicit bootstrap config should resolve");

    let execution = run_local_session(&config).expect("bootstrap run loop should succeed");

    assert_eq!(execution.snapshot.runtime_state, RuntimeState::Ready);
    assert_eq!(
        execution.stopped_snapshot.runtime_state,
        RuntimeState::Stopped
    );
    assert_eq!(
        execution.stopped_snapshot.manifest_contract.freshness.state,
        simard::FreshnessState::Stale
    );
}

#[test]
fn bootstrap_assembly_supports_multiple_builtin_manifest_base_types() {
    for base_type in ["local-harness", "rusty-clawd", "copilot-sdk"] {
        let config = BootstrapConfig::resolve(BootstrapInputs {
            prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
            objective: Some(format!("exercise base type handling for {base_type}")),
            identity: Some("simard-engineer".to_string()),
            base_type: Some(base_type.to_string()),
            topology: Some("single-process".to_string()),
            ..BootstrapInputs::default()
        })
        .expect("explicit bootstrap config should resolve");

        let execution =
            run_local_session(&config).expect("builtin manifest base type should execute");

        assert_eq!(
            execution.snapshot.selected_base_type,
            BaseTypeId::new(base_type)
        );
        assert_eq!(execution.snapshot.adapter_backend.identity, base_type);
        assert_eq!(execution.snapshot.topology, RuntimeTopology::SingleProcess);
    }
}

#[test]
fn bootstrap_assembly_surfaces_identity_base_type_mismatches_explicitly() {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
        objective: Some("exercise identity base type validation".to_string()),
        identity: Some("simard-engineer".to_string()),
        base_type: Some("meeting-bot".to_string()),
        topology: Some("single-process".to_string()),
        ..BootstrapInputs::default()
    })
    .expect("explicit bootstrap config should resolve");

    let error = match assemble_local_runtime(&config) {
        Ok(_) => panic!("identity/base type mismatches should fail runtime assembly"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        SimardError::UnsupportedBaseType {
            identity: "simard-engineer".to_string(),
            base_type: "meeting-bot".to_string(),
        }
    );
}

#[test]
fn bootstrap_assembly_surfaces_unsupported_topologies_explicitly() {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
        objective: Some("exercise unsupported topology handling".to_string()),
        identity: Some("simard-engineer".to_string()),
        base_type: Some("local-harness".to_string()),
        topology: Some("distributed".to_string()),
        ..BootstrapInputs::default()
    })
    .expect("explicit bootstrap config should resolve");

    let error = match assemble_local_runtime(&config) {
        Ok(_) => panic!("unsupported topology should fail runtime assembly"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        SimardError::UnsupportedTopology {
            base_type: "local-harness".to_string(),
            topology: RuntimeTopology::Distributed,
        }
    );
}
