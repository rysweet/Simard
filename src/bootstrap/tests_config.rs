use std::path::PathBuf;

use super::config::BootstrapConfig;
use crate::bootstrap::test_support::TestDir;
use crate::bootstrap::{BootstrapInputs, BootstrapMode, ConfigValueSource};
use crate::error::SimardError;
use crate::runtime::RuntimeTopology;

// ── BootstrapConfig::resolve ──

#[test]
fn resolve_builtin_defaults_produces_valid_config() {
    let inputs = BootstrapInputs {
        mode: Some("builtin-defaults".to_string()),
        ..Default::default()
    };
    let config = BootstrapConfig::resolve(inputs).unwrap();
    assert_eq!(config.mode, BootstrapMode::BuiltinDefaults);
    assert_eq!(config.identity, "simard-engineer");
    assert_eq!(config.selected_base_type.value.as_str(), "local-harness");
}

#[test]
fn resolve_explicit_config_without_prompt_root_fails() {
    let inputs = BootstrapInputs {
        mode: Some("explicit-config".to_string()),
        ..Default::default()
    };
    let err = BootstrapConfig::resolve(inputs).unwrap_err();
    match err {
        SimardError::MissingRequiredConfig { key, .. } => {
            assert_eq!(key, "SIMARD_PROMPT_ROOT");
        }
        other => panic!("expected MissingRequiredConfig, got {other:?}"),
    }
}

#[test]
fn resolve_explicit_config_without_objective_fails() {
    let inputs = BootstrapInputs {
        mode: Some("explicit-config".to_string()),
        prompt_root: Some(PathBuf::from("/some/path")),
        ..Default::default()
    };
    let err = BootstrapConfig::resolve(inputs).unwrap_err();
    match err {
        SimardError::MissingRequiredConfig { key, .. } => {
            assert_eq!(key, "SIMARD_OBJECTIVE");
        }
        other => panic!("expected MissingRequiredConfig, got {other:?}"),
    }
}

#[test]
fn resolve_explicit_config_without_base_type_fails() {
    let inputs = BootstrapInputs {
        mode: Some("explicit-config".to_string()),
        prompt_root: Some(PathBuf::from("/some/path")),
        objective: Some("test".to_string()),
        ..Default::default()
    };
    let err = BootstrapConfig::resolve(inputs).unwrap_err();
    match err {
        SimardError::MissingRequiredConfig { key, .. } => {
            assert_eq!(key, "SIMARD_BASE_TYPE");
        }
        other => panic!("expected MissingRequiredConfig, got {other:?}"),
    }
}

#[test]
fn resolve_explicit_config_without_topology_fails() {
    let inputs = BootstrapInputs {
        mode: Some("explicit-config".to_string()),
        prompt_root: Some(PathBuf::from("/some/path")),
        objective: Some("test".to_string()),
        base_type: Some("local-harness".to_string()),
        ..Default::default()
    };
    let err = BootstrapConfig::resolve(inputs).unwrap_err();
    match err {
        SimardError::MissingRequiredConfig { key, .. } => {
            assert_eq!(key, "SIMARD_RUNTIME_TOPOLOGY");
        }
        other => panic!("expected MissingRequiredConfig, got {other:?}"),
    }
}

#[test]
fn resolve_explicit_config_without_state_root_fails() {
    let inputs = BootstrapInputs {
        mode: Some("explicit-config".to_string()),
        prompt_root: Some(PathBuf::from("/some/path")),
        objective: Some("test".to_string()),
        base_type: Some("local-harness".to_string()),
        topology: Some("single-process".to_string()),
        ..Default::default()
    };
    let err = BootstrapConfig::resolve(inputs).unwrap_err();
    match err {
        SimardError::MissingRequiredConfig { key, .. } => {
            assert_eq!(key, "SIMARD_STATE_ROOT");
        }
        other => panic!("expected MissingRequiredConfig, got {other:?}"),
    }
}

#[test]
fn resolve_explicit_config_without_identity_fails() {
    let temp_dir = TestDir::new("simard-resolve-test");
    let inputs = BootstrapInputs {
        mode: Some("explicit-config".to_string()),
        prompt_root: Some(PathBuf::from("/some/path")),
        objective: Some("test".to_string()),
        base_type: Some("local-harness".to_string()),
        topology: Some("single-process".to_string()),
        state_root: Some(temp_dir.path().to_path_buf()),
        ..Default::default()
    };
    let err = BootstrapConfig::resolve(inputs).unwrap_err();
    match err {
        SimardError::MissingRequiredConfig { key, .. } => {
            assert_eq!(key, "SIMARD_IDENTITY");
        }
        other => panic!("expected MissingRequiredConfig, got {other:?}"),
    }
}

#[test]
fn resolve_full_explicit_config_succeeds() {
    let temp_dir = TestDir::new("simard-resolve-full");
    let inputs = BootstrapInputs {
        mode: Some("explicit-config".to_string()),
        prompt_root: Some(PathBuf::from("/some/path")),
        objective: Some("my-objective".to_string()),
        base_type: Some("local-harness".to_string()),
        topology: Some("single-process".to_string()),
        state_root: Some(temp_dir.path().to_path_buf()),
        identity: Some("my-identity".to_string()),
    };
    let config = BootstrapConfig::resolve(inputs).unwrap();
    assert_eq!(config.mode, BootstrapMode::ExplicitConfig);
    assert_eq!(config.identity, "my-identity");
    assert_eq!(config.objective.value, "my-objective");
    assert_eq!(config.selected_base_type.value.as_str(), "local-harness");
    assert_eq!(config.topology.value, RuntimeTopology::SingleProcess);
    assert_eq!(config.prompt_root.value, PathBuf::from("/some/path"));
    assert!(matches!(
        config.prompt_root.source,
        ConfigValueSource::Environment(_)
    ));
    assert!(matches!(
        config.objective.source,
        ConfigValueSource::Environment(_)
    ));
}

// ── BootstrapConfig path methods ──

#[test]
fn config_path_methods_use_state_root() {
    let temp_dir = TestDir::new("simard-paths-test");
    let inputs = BootstrapInputs {
        mode: Some("builtin-defaults".to_string()),
        state_root: Some(temp_dir.path().to_path_buf()),
        ..Default::default()
    };
    let config = BootstrapConfig::resolve(inputs).unwrap();
    assert!(
        config
            .memory_store_path()
            .to_string_lossy()
            .contains("memory_records.json")
    );
    assert!(
        config
            .evidence_store_path()
            .to_string_lossy()
            .contains("evidence_records.json")
    );
    assert!(
        config
            .goal_store_path()
            .to_string_lossy()
            .contains("goal_records.json")
    );
    assert!(
        config
            .handoff_store_path()
            .to_string_lossy()
            .contains("latest_handoff.json")
    );
}

#[test]
fn state_root_path_returns_state_root_ref() {
    let inputs = BootstrapInputs {
        mode: Some("builtin-defaults".to_string()),
        ..Default::default()
    };
    let config = BootstrapConfig::resolve(inputs).unwrap();
    assert!(!config.state_root_path().as_os_str().is_empty());
}

// ── manifest_precedence ──

#[test]
fn manifest_precedence_returns_expected_entries() {
    let inputs = BootstrapInputs {
        mode: Some("builtin-defaults".to_string()),
        ..Default::default()
    };
    let config = BootstrapConfig::resolve(inputs).unwrap();
    let prec = config.manifest_precedence();
    assert!(prec.len() >= 7);
    assert!(prec.iter().any(|s| s.starts_with("mode:")));
    assert!(prec.iter().any(|s| s.starts_with("identity:")));
    assert!(prec.iter().any(|s| s.starts_with("base-type:")));
    assert!(prec.iter().any(|s| s.starts_with("topology:")));
    assert!(prec.iter().any(|s| s.starts_with("prompt-root:")));
    assert!(prec.iter().any(|s| s.starts_with("state-root:")));
    assert!(prec.iter().any(|s| s.starts_with("objective:")));
}
