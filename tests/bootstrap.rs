use std::fs;
use std::path::PathBuf;

use simard::{
    BaseTypeId, BootstrapConfig, BootstrapInputs, BootstrapMode, BuiltinIdentityLoader,
    ConfigValueSource, IdentityLoadRequest, IdentityLoader, ManifestContract, MemoryScope,
    Provenance, ReflectiveRuntime, RuntimeState, RuntimeTopology, SimardError,
    assemble_local_runtime, assemble_local_runtime_from_handoff, bootstrap_entrypoint,
    latest_local_handoff, run_local_session,
};

fn state_root(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-state")
        .join(name)
}

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
        config.state_root.source,
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
    assert_eq!(
        config.state_root.value,
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/simard-state")
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
        state_root: Some(state_root("missing-identity")),
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
        state_root: Some(state_root("missing-base-type")),
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
        state_root: Some(state_root("missing-topology")),
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
        state_root: Some(state_root("invalid-topology")),
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
fn invalid_config_display_redacts_raw_values() {
    let error = SimardError::InvalidConfigValue {
        key: "SIMARD_RUNTIME_TOPOLOGY".to_string(),
        value: "mystery-mesh".to_string(),
        help: "expected 'single-process', 'multi-process', or 'distributed'".to_string(),
    };

    let rendered = error.to_string();
    assert!(rendered.contains("SIMARD_RUNTIME_TOPOLOGY"));
    assert!(rendered.contains("expected 'single-process'"));
    assert!(!rendered.contains("mystery-mesh"));
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
    assert!(manifest.components.is_empty());
}

#[test]
fn bootstrap_assembly_produces_truthful_manifest_metadata() {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
        objective: Some("exercise bootstrap assembly".to_string()),
        identity: Some("simard-engineer".to_string()),
        base_type: Some("local-harness".to_string()),
        topology: Some("single-process".to_string()),
        state_root: Some(state_root("assembly")),
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
    assert!(snapshot.identity_components.is_empty());
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
fn main_reports_selected_base_type_and_runtime_implementation_separately() {
    let main_rs = include_str!("../src/main.rs");

    assert!(main_rs.contains("Bootstrap selection:"));
    assert!(main_rs.contains("Adapter implementation:"));
}

#[test]
fn bootstrap_run_local_session_executes_the_cli_lifecycle() {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
        objective: Some("exercise the bootstrap run loop".to_string()),
        identity: Some("simard-engineer".to_string()),
        base_type: Some("local-harness".to_string()),
        topology: Some("single-process".to_string()),
        state_root: Some(state_root("run-loop")),
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
fn bootstrap_persists_durable_state_and_restores_latest_handoff() {
    let state_root = state_root("durable-restore");
    if state_root.exists() {
        fs::remove_dir_all(&state_root).expect("old test state should be removable");
    }

    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
        objective: Some("exercise durable bootstrap persistence".to_string()),
        state_root: Some(state_root.clone()),
        identity: Some("simard-engineer".to_string()),
        base_type: Some("local-harness".to_string()),
        topology: Some("single-process".to_string()),
        ..BootstrapInputs::default()
    })
    .expect("explicit bootstrap config should resolve");

    let execution = run_local_session(&config).expect("bootstrap run loop should succeed");
    assert!(config.memory_store_path().is_file());
    assert!(config.evidence_store_path().is_file());
    assert!(config.handoff_store_path().is_file());

    let snapshot = latest_local_handoff(&config)
        .expect("durable handoff lookup should succeed")
        .expect("a durable handoff snapshot should exist after the run");
    assert_eq!(
        snapshot.session.as_ref().map(|session| session.phase),
        Some(simard::SessionPhase::Complete)
    );
    assert_eq!(snapshot.memory_records.len(), 2);
    assert_eq!(snapshot.evidence_records.len(), 4);

    let restored = assemble_local_runtime_from_handoff(&config, snapshot)
        .expect("restored runtime should compose");
    let restored_snapshot = restored
        .snapshot()
        .expect("restored snapshot should succeed");
    assert_eq!(restored_snapshot.runtime_state, RuntimeState::Initializing);
    assert_eq!(
        restored_snapshot.session_phase,
        Some(simard::SessionPhase::Complete)
    );
    assert_eq!(restored_snapshot.memory_records, 2);
    assert_eq!(restored_snapshot.evidence_records, 4);
    assert_eq!(
        restored_snapshot.memory_backend.identity,
        "memory::json-file-store"
    );
    assert_eq!(
        restored_snapshot.evidence_backend.identity,
        "evidence::json-file-store"
    );
    assert_eq!(
        restored_snapshot.handoff_backend.identity,
        "handoff::json-file-store"
    );
    assert_eq!(
        execution.stopped_snapshot.runtime_state,
        RuntimeState::Stopped
    );
}

#[test]
fn bootstrap_supports_composite_identity_execution() {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
        objective: Some("exercise the composite engineer loop".to_string()),
        identity: Some("simard-composite-engineer".to_string()),
        base_type: Some("local-harness".to_string()),
        topology: Some("single-process".to_string()),
        state_root: Some(state_root("composite")),
        ..BootstrapInputs::default()
    })
    .expect("explicit composite bootstrap config should resolve");

    let execution = run_local_session(&config).expect("composite identity should execute");

    assert_eq!(
        execution.snapshot.identity_name,
        "simard-composite-engineer"
    );
    assert_eq!(
        execution.snapshot.identity_components,
        vec![
            "simard-engineer".to_string(),
            "simard-meeting".to_string(),
            "simard-gym".to_string()
        ]
    );
    assert_eq!(execution.snapshot.topology, RuntimeTopology::SingleProcess);
    assert_eq!(execution.snapshot.adapter_backend.identity, "local-harness");
    assert_eq!(
        execution.outcome.session.phase,
        simard::SessionPhase::Complete
    );
}

#[test]
fn bootstrap_meeting_mode_persists_structured_decision_memory() {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
        objective: Some(
            "agenda: review next Simard milestone\nupdate: durable memory shipped in PR 29\ndecision: prioritize meeting mode before remote orchestration\nrisk: workflow runner keeps drifting worktrees\nnext-step: add outside-in meeting probe\nopen-question: when should meeting decisions auto-influence engineer planning?"
                .to_string(),
        ),
        identity: Some("simard-meeting".to_string()),
        base_type: Some("local-harness".to_string()),
        topology: Some("single-process".to_string()),
        state_root: Some(state_root("meeting-mode")),
        ..BootstrapInputs::default()
    })
    .expect("meeting bootstrap config should resolve");

    let execution = run_local_session(&config).expect("meeting mode should execute");
    let exported = latest_local_handoff(&config)
        .expect("meeting handoff lookup should succeed")
        .expect("meeting handoff should exist");
    let decision_records = exported
        .memory_records
        .iter()
        .filter(|record| record.scope == MemoryScope::Decision)
        .collect::<Vec<_>>();

    assert_eq!(execution.snapshot.identity_name, "simard-meeting");
    assert_eq!(
        execution.snapshot.agent_program_backend.identity,
        "agent-program::meeting-facilitator"
    );
    assert_eq!(decision_records.len(), 1);
    assert!(
        decision_records[0]
            .value
            .contains("prioritize meeting mode before remote orchestration"),
        "decision memory should keep the explicit decision"
    );
    assert!(
        decision_records[0]
            .value
            .contains("workflow runner keeps drifting worktrees"),
        "decision memory should keep the explicit risk"
    );
    assert!(
        decision_records[0]
            .value
            .contains("add outside-in meeting probe"),
        "decision memory should keep the next step"
    );
    assert!(
        execution
            .outcome
            .reflection
            .summary
            .contains("captured 1 decisions, 1 risks, 1 next steps, and 1 open questions"),
        "meeting reflection should expose the structured capture counts"
    );
}

#[test]
fn bootstrap_assembly_supports_multiple_builtin_manifest_base_types() {
    for base_type in [
        "local-harness",
        "terminal-shell",
        "rusty-clawd",
        "copilot-sdk",
    ] {
        let config = BootstrapConfig::resolve(BootstrapInputs {
            prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
            objective: Some(match base_type {
                "terminal-shell" => "working-directory: .\ncommand: pwd\ncommand: printf \"terminal-bootstrap-ok\\n\"".to_string(),
                _ => format!("exercise base type handling for {base_type}"),
            }),
            identity: Some("simard-engineer".to_string()),
            base_type: Some(base_type.to_string()),
            topology: Some("single-process".to_string()),
            state_root: Some(state_root(base_type)),
            ..BootstrapInputs::default()
        })
        .expect("explicit bootstrap config should resolve");

        let execution =
            run_local_session(&config).expect("builtin manifest base type should execute");

        assert_eq!(
            execution.snapshot.selected_base_type,
            BaseTypeId::new(base_type)
        );
        let expected_backend = match base_type {
            "terminal-shell" => "terminal-shell::local-pty",
            "rusty-clawd" => "rusty-clawd::session-backend",
            _ => "local-harness",
        };
        assert_eq!(
            execution.snapshot.adapter_backend.identity,
            expected_backend
        );
        assert_eq!(execution.snapshot.topology, RuntimeTopology::SingleProcess);
        assert!(
            execution
                .snapshot
                .adapter_backend
                .provenance
                .locator
                .contains(base_type),
            "adapter provenance should keep the selected alias visible"
        );
        assert!(
            execution
                .outcome
                .execution_summary
                .contains(expected_backend),
            "execution summary should describe the canonical v1 implementation"
        );
        assert!(
            !execution
                .outcome
                .execution_summary
                .contains(config.objective.value.as_str()),
            "execution summaries should not persist the raw objective"
        );
    }
}

#[test]
fn bootstrap_supports_terminal_shell_execution() {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
        objective: Some(
            "working-directory: .\ncommand: pwd\ncommand: printf \"terminal-foundation-ok\\n\""
                .to_string(),
        ),
        identity: Some("simard-engineer".to_string()),
        base_type: Some("terminal-shell".to_string()),
        topology: Some("single-process".to_string()),
        state_root: Some(state_root("terminal-shell-execution")),
        ..BootstrapInputs::default()
    })
    .expect("terminal-shell config should resolve");

    let execution =
        run_local_session(&config).expect("terminal-shell bootstrap run loop should succeed");
    let exported = latest_local_handoff(&config)
        .expect("terminal handoff should load")
        .expect("terminal handoff should exist");

    assert_eq!(
        execution.snapshot.adapter_backend.identity,
        "terminal-shell::local-pty"
    );
    assert!(
        execution
            .snapshot
            .adapter_capabilities
            .contains(&"terminal-session".to_string()),
        "reflection should expose the terminal-session capability"
    );
    assert_eq!(
        execution.snapshot.adapter_supported_topologies,
        vec!["single-process".to_string()]
    );
    assert!(
        execution
            .outcome
            .execution_summary
            .contains("terminal-shell::local-pty"),
        "execution summary should report the terminal-shell backend"
    );
    let terminal_evidence = exported
        .evidence_records
        .iter()
        .map(|record| record.detail.as_str())
        .collect::<Vec<_>>();
    assert!(
        terminal_evidence
            .iter()
            .any(|detail| detail == &"terminal-command-count=2"),
        "terminal evidence should report the interactive command count"
    );
    assert!(
        terminal_evidence
            .iter()
            .any(|detail| detail.contains("terminal-foundation-ok")),
        "terminal transcript evidence should keep a preview of real terminal output"
    );
}

#[test]
fn bootstrap_supports_rusty_clawd_multi_process_execution() {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
        objective: Some("exercise multi-process rusty-clawd bootstrap".to_string()),
        identity: Some("simard-engineer".to_string()),
        base_type: Some("rusty-clawd".to_string()),
        topology: Some("multi-process".to_string()),
        state_root: Some(state_root("rusty-clawd-multi-process")),
        ..BootstrapInputs::default()
    })
    .expect("multi-process bootstrap config should resolve");

    let execution =
        run_local_session(&config).expect("loopback multi-process bootstrap should execute");

    assert_eq!(execution.snapshot.topology, RuntimeTopology::MultiProcess);
    assert_eq!(
        execution.snapshot.runtime_node.to_string(),
        "node-loopback-mesh"
    );
    assert_eq!(
        execution.snapshot.mailbox_address.to_string(),
        "loopback://node-loopback-mesh"
    );
    assert_eq!(
        execution.snapshot.adapter_backend.identity,
        "rusty-clawd::session-backend"
    );
    assert_eq!(
        execution.snapshot.topology_backend.identity,
        "topology::loopback-mesh"
    );
    assert_eq!(
        execution.snapshot.transport_backend.identity,
        "transport::loopback-mailbox"
    );
    assert_eq!(
        execution.snapshot.supervisor_backend.identity,
        "supervisor::coordinated"
    );
}

#[test]
fn bootstrap_assembly_surfaces_identity_base_type_mismatches_explicitly() {
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")),
        objective: Some("exercise identity base type validation".to_string()),
        identity: Some("simard-engineer".to_string()),
        base_type: Some("meeting-bot".to_string()),
        topology: Some("single-process".to_string()),
        state_root: Some(state_root("identity-base-mismatch")),
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
        state_root: Some(state_root("unsupported-topology")),
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
