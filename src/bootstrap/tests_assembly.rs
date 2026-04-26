use super::LOCAL_BASE_TYPE;
use super::assembly::{builtin_base_type_registry_for_manifest, register_builtin_base_type};
use crate::base_type_rustyclawd::RustyClawdAdapter;
use crate::base_types::{BaseTypeFactory, BaseTypeId};
use crate::identity::{
    BuiltinIdentityLoader, IdentityLoadRequest, IdentityLoader, ManifestContract,
};
use crate::metadata::{Freshness, Provenance};
use crate::runtime::BaseTypeRegistry;

#[test]
fn builtin_adapter_catalog_covers_manifest_advertised_base_types() {
    let manifest = BuiltinIdentityLoader
        .load(&IdentityLoadRequest::new(
            "simard-engineer",
            env!("CARGO_PKG_VERSION"),
            ManifestContract::new(
                crate::bootstrap_entrypoint(),
                "bootstrap-config -> identity-loader -> runtime-ports -> local-runtime",
                vec!["tests:bootstrap-catalog".to_string()],
                Provenance::new("test", "bootstrap::catalog"),
                Freshness::now().expect("freshness should be observable"),
            )
            .expect("contract should be valid"),
        ))
        .expect("builtin identity should load");

    let registry =
        builtin_base_type_registry_for_manifest(&manifest).expect("registry should build");
    let local = registry
        .get(&BaseTypeId::new("local-harness"))
        .expect("local harness should be registered");
    let rusty = registry
        .get(&BaseTypeId::new("rusty-clawd"))
        .expect("rusty-clawd should be registered");
    let copilot = registry
        .get(&BaseTypeId::new("copilot-sdk"))
        .expect("copilot-sdk should be registered");
    let claude_sdk = registry
        .get(&BaseTypeId::new("claude-agent-sdk"))
        .expect("claude-agent-sdk should be registered");
    let ms_agent = registry
        .get(&BaseTypeId::new("ms-agent-framework"))
        .expect("ms-agent-framework should be registered");

    assert_eq!(local.descriptor().backend.identity, LOCAL_BASE_TYPE);
    assert_eq!(
        copilot.descriptor().backend.identity,
        "copilot-sdk::pty-session"
    );
    assert_eq!(
        rusty.descriptor().backend.identity,
        RustyClawdAdapter::registered("rusty-clawd")
            .expect("rusty-clawd adapter should initialize")
            .descriptor()
            .backend
            .identity
    );
    assert_eq!(
        claude_sdk.descriptor().backend.identity,
        "claude-agent-sdk::session-backend"
    );
    assert_eq!(
        ms_agent.descriptor().backend.identity,
        "ms-agent-framework::session-backend"
    );
}

// ── register_builtin_base_type ──

#[test]
fn register_unknown_base_type_does_not_error() {
    let mut registry = BaseTypeRegistry::default();
    let result = register_builtin_base_type(&mut registry, &BaseTypeId::new("nonexistent"));
    assert!(
        result.is_ok(),
        "unknown base type should be silently ignored"
    );
}

#[test]
fn register_local_harness_base_type_succeeds() {
    let mut registry = BaseTypeRegistry::default();
    let result = register_builtin_base_type(&mut registry, &BaseTypeId::new("local-harness"));
    assert!(result.is_ok());
    assert!(registry.get(&BaseTypeId::new("local-harness")).is_some());
}

#[test]
fn register_rusty_clawd_base_type_succeeds() {
    let mut registry = BaseTypeRegistry::default();
    let result = register_builtin_base_type(&mut registry, &BaseTypeId::new("rusty-clawd"));
    assert!(result.is_ok());
    assert!(registry.get(&BaseTypeId::new("rusty-clawd")).is_some());
}

#[test]
fn register_terminal_shell_base_type_succeeds() {
    let mut registry = BaseTypeRegistry::default();
    let result = register_builtin_base_type(&mut registry, &BaseTypeId::new("terminal-shell"));
    assert!(result.is_ok());
    assert!(registry.get(&BaseTypeId::new("terminal-shell")).is_some());
}
