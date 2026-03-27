use std::path::PathBuf;

use simard::{
    BootstrapConfig, BootstrapInputs, BootstrapMode, BuiltinIdentityLoader, ConfigValueSource,
    IdentityLoadRequest, IdentityLoader, SimardError,
};

#[test]
fn bootstrap_requires_explicit_prompt_root_and_objective_by_default() {
    let error = BootstrapConfig::resolve(BootstrapInputs::default()).unwrap_err();

    assert_eq!(
        error,
        SimardError::MissingRequiredConfig {
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
    assert_eq!(
        config.prompt_root.source,
        ConfigValueSource::ExplicitOptIn("SIMARD_BOOTSTRAP_MODE")
    );
    assert_eq!(
        config.objective.source,
        ConfigValueSource::ExplicitOptIn("SIMARD_BOOTSTRAP_MODE")
    );
    assert_eq!(
        config.prompt_root.value,
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets")
    );
    assert_eq!(config.objective.value, "bootstrap the Simard engineer loop");
}

#[test]
fn builtin_identity_loader_returns_manifest_contract_metadata() {
    let manifest = BuiltinIdentityLoader
        .load(&IdentityLoadRequest::new(
            "simard-engineer",
            "0.1.0",
            vec![
                "mode:explicit-config".to_string(),
                "prompt-root:env:SIMARD_PROMPT_ROOT".to_string(),
            ],
        ))
        .expect("builtin identity should load");

    assert_eq!(manifest.contract.entrypoint, "src/main.rs");
    assert_eq!(
        manifest.contract.composition,
        "bootstrap-config -> manifest-loader -> runtime-ports -> local-runtime"
    );
    assert_eq!(
        manifest.contract.precedence,
        vec![
            "mode:explicit-config".to_string(),
            "prompt-root:env:SIMARD_PROMPT_ROOT".to_string(),
        ]
    );
    assert_eq!(manifest.provenance.source, "builtin");
}
