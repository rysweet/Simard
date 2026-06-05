//! Extended unit tests for the `identity` module.
//!
//! Covers validation edge cases, trimming behaviour, request/contract
//! propagation, composite identity invariants, and serde boundaries
//! that the existing inline tests do not exercise.
//! No `skip_if_no_llm_provider` — every test here runs deterministically.

use super::*;
use crate::base_types::{BaseTypeCapability, BaseTypeId, capability_set};
use crate::memory::MemoryScope;
use crate::metadata::{Freshness, Provenance};
use crate::prompt_assets::PromptAssetRef;
use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_contract() -> ManifestContract {
    ManifestContract::new(
        "test::entrypoint",
        "a -> b",
        vec!["key:value".to_string()],
        Provenance::new("test-source", "test-locator"),
        Freshness::now().unwrap(),
    )
    .unwrap()
}

fn simple_manifest(name: &str) -> IdentityManifest {
    IdentityManifest::new(
        name,
        "1.0",
        vec![],
        vec![BaseTypeId::new("local-harness")],
        capability_set([]),
        OperatingMode::Engineer,
        MemoryPolicy::default(),
        test_contract(),
    )
    .unwrap()
}

// ===========================================================================
// OperatingMode — serde boundary
// ===========================================================================

#[test]
fn operating_mode_display_matches_serde_for_all_variants() {
    let modes = [
        OperatingMode::Engineer,
        OperatingMode::Meeting,
        OperatingMode::Curator,
        OperatingMode::Improvement,
        OperatingMode::Gym,
        OperatingMode::Orchestrator,
    ];
    for mode in modes {
        let display = mode.to_string();
        let json = serde_json::to_string(&mode).unwrap();
        // serde produces `"engineer"`, Display produces `engineer`
        assert_eq!(
            format!("\"{display}\""),
            json,
            "Display and serde should agree for {mode:?}"
        );
    }
}

#[test]
fn operating_mode_deserialization_rejects_unknown_string() {
    let result: Result<OperatingMode, _> = serde_json::from_str("\"unknown-mode\"");
    assert!(result.is_err(), "unknown mode should fail deserialization");
}

#[test]
fn operating_mode_deserialization_rejects_empty_string() {
    let result: Result<OperatingMode, _> = serde_json::from_str("\"\"");
    assert!(result.is_err(), "empty string should fail deserialization");
}

// ===========================================================================
// MemoryPolicy — scope inequality
// ===========================================================================

#[test]
fn memory_policy_different_scopes_are_not_equal() {
    let a = MemoryPolicy {
        allow_project_writes: false,
        summary_scope: MemoryScope::SessionSummary,
    };
    let b = MemoryPolicy {
        allow_project_writes: false,
        summary_scope: MemoryScope::Decision,
    };
    assert_ne!(a, b, "policies with different scopes should not be equal");
}

#[test]
fn memory_policy_same_values_are_equal() {
    let a = MemoryPolicy::default();
    let b = MemoryPolicy::default();
    assert_eq!(a, b);
}

// ===========================================================================
// ManifestContract — trimming and edge cases
// ===========================================================================

#[test]
fn contract_trims_whitespace_from_entrypoint() {
    let contract = ManifestContract::new(
        "  test::entrypoint  ",
        "a -> b",
        vec!["key:value".to_string()],
        Provenance::new("src", "loc"),
        Freshness::now().unwrap(),
    )
    .unwrap();
    assert_eq!(contract.entrypoint, "test::entrypoint");
}

#[test]
fn contract_trims_whitespace_from_composition() {
    let contract = ManifestContract::new(
        "test::entry",
        "  a -> b  ",
        vec!["key:value".to_string()],
        Provenance::new("src", "loc"),
        Freshness::now().unwrap(),
    )
    .unwrap();
    assert_eq!(contract.composition, "a -> b");
}

#[test]
fn contract_trims_whitespace_from_precedence_entries() {
    let contract = ManifestContract::new(
        "test::entry",
        "a -> b",
        vec!["  layer:test  ".to_string()],
        Provenance::new("src", "loc"),
        Freshness::now().unwrap(),
    )
    .unwrap();
    assert_eq!(contract.precedence[0], "layer:test");
}

#[test]
fn contract_rejects_duplicate_precedence_after_trimming() {
    let err = ManifestContract::new(
        "test::entry",
        "a -> b",
        vec!["key:value".to_string(), "  key:value  ".to_string()],
        Provenance::new("src", "loc"),
        Freshness::now().unwrap(),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("duplicate"),
        "should detect duplicate after trimming: {err}"
    );
}

#[test]
fn contract_accepts_precedence_with_colon_at_end() {
    // "key:" has a colon so should pass the colon check
    let result = ManifestContract::new(
        "test::entry",
        "a -> b",
        vec!["key:".to_string()],
        Provenance::new("src", "loc"),
        Freshness::now().unwrap(),
    );
    assert!(
        result.is_ok(),
        "precedence 'key:' contains ':' and should pass"
    );
}

#[test]
fn contract_accepts_precedence_with_colon_at_start() {
    let result = ManifestContract::new(
        "test::entry",
        "a -> b",
        vec![":value".to_string()],
        Provenance::new("src", "loc"),
        Freshness::now().unwrap(),
    );
    assert!(
        result.is_ok(),
        "precedence ':value' contains ':' and should pass"
    );
}

#[test]
fn contract_trims_provenance_fields() {
    let contract = ManifestContract::new(
        "test::entry",
        "a -> b",
        vec!["key:value".to_string()],
        Provenance::new("  my-source  ", "  my-locator  "),
        Freshness::now().unwrap(),
    )
    .unwrap();
    assert_eq!(contract.provenance.source, "my-source");
    assert_eq!(contract.provenance.locator, "my-locator");
}

#[test]
fn contract_multiple_valid_precedence_entries() {
    let contract = ManifestContract::new(
        "test::entry",
        "a -> b",
        vec![
            "layer:base".to_string(),
            "identity:simard".to_string(),
            "env:prod".to_string(),
        ],
        Provenance::new("src", "loc"),
        Freshness::now().unwrap(),
    )
    .unwrap();
    assert_eq!(contract.precedence.len(), 3);
}

// ===========================================================================
// IdentityManifest::new — field initialization
// ===========================================================================

#[test]
fn manifest_new_initializes_components_to_empty_vec() {
    let manifest = simple_manifest("test");
    assert!(
        manifest.components.is_empty(),
        "new() should not populate components"
    );
}

#[test]
fn manifest_new_stores_name_and_version() {
    let manifest = IdentityManifest::new(
        "my-identity",
        "2.3.4",
        vec![],
        vec![BaseTypeId::new("local-harness")],
        capability_set([]),
        OperatingMode::Gym,
        MemoryPolicy::default(),
        test_contract(),
    )
    .unwrap();
    assert_eq!(manifest.name, "my-identity");
    assert_eq!(manifest.version, "2.3.4");
    assert_eq!(manifest.default_mode, OperatingMode::Gym);
}

// ===========================================================================
// IdentityManifest::with_components — trimming
// ===========================================================================

#[test]
fn manifest_with_components_trims_whitespace() {
    let manifest = simple_manifest("parent")
        .with_components(["  child-a  ", "  child-b  "])
        .unwrap();
    assert_eq!(manifest.components, vec!["child-a", "child-b"]);
}

#[test]
fn manifest_with_components_many_components_succeeds() {
    let names: Vec<String> = (0..10).map(|i| format!("child-{i}")).collect();
    let manifest = simple_manifest("parent")
        .with_components(names.clone())
        .unwrap();
    assert_eq!(manifest.components.len(), 10);
    assert_eq!(manifest.components, names);
}

// ===========================================================================
// IdentityManifest::compose — single component and propagation
// ===========================================================================

#[test]
fn compose_single_component_preserves_identity() {
    let child = simple_manifest("only-child");
    let composed = IdentityManifest::compose(
        "parent",
        "3.0",
        vec![child.clone()],
        OperatingMode::Meeting,
        test_contract(),
    )
    .unwrap();
    assert_eq!(composed.name, "parent");
    assert_eq!(composed.version, "3.0");
    assert_eq!(composed.default_mode, OperatingMode::Meeting);
    assert_eq!(composed.components, vec!["only-child"]);
    assert_eq!(composed.memory_policy, child.memory_policy);
}

#[test]
fn compose_three_components_unions_capabilities() {
    let mut c1 = simple_manifest("c1");
    c1.required_capabilities = capability_set([BaseTypeCapability::PromptAssets]);
    let mut c2 = simple_manifest("c2");
    c2.required_capabilities = capability_set([BaseTypeCapability::Memory]);
    let mut c3 = simple_manifest("c3");
    c3.required_capabilities = capability_set([
        BaseTypeCapability::Evidence,
        BaseTypeCapability::PromptAssets,
    ]);

    let composed = IdentityManifest::compose(
        "three-way",
        "1.0",
        vec![c1, c2, c3],
        OperatingMode::Engineer,
        test_contract(),
    )
    .unwrap();

    assert!(
        composed
            .required_capabilities
            .contains(&BaseTypeCapability::PromptAssets)
    );
    assert!(
        composed
            .required_capabilities
            .contains(&BaseTypeCapability::Memory)
    );
    assert!(
        composed
            .required_capabilities
            .contains(&BaseTypeCapability::Evidence)
    );
    assert_eq!(composed.required_capabilities.len(), 3);
}

#[test]
fn compose_preserves_contract_from_caller() {
    let child = simple_manifest("child");
    let contract = ManifestContract::new(
        "custom::entry",
        "x -> y",
        vec!["layer:custom".to_string()],
        Provenance::new("custom-src", "custom-loc"),
        Freshness::now().unwrap(),
    )
    .unwrap();
    let composed = IdentityManifest::compose(
        "parent",
        "1.0",
        vec![child],
        OperatingMode::Engineer,
        contract.clone(),
    )
    .unwrap();
    assert_eq!(composed.contract.entrypoint, "custom::entry");
    assert_eq!(composed.contract.composition, "x -> y");
}

// ===========================================================================
// compose_with_precedence — conflict logging
// ===========================================================================

#[test]
fn compose_with_precedence_two_manifests_with_conflicts() {
    let m1 = IdentityManifest::new(
        "high",
        "1.0",
        vec![PromptAssetRef::new("shared", "a.md")],
        vec![BaseTypeId::new("shared-bt")],
        capability_set([BaseTypeCapability::Memory]),
        OperatingMode::Engineer,
        MemoryPolicy::default(),
        test_contract(),
    )
    .unwrap();
    let m2 = IdentityManifest::new(
        "low",
        "1.0",
        vec![PromptAssetRef::new("shared", "b.md")],
        vec![BaseTypeId::new("shared-bt")],
        capability_set([BaseTypeCapability::Evidence]),
        OperatingMode::Engineer,
        MemoryPolicy::default(),
        test_contract(),
    )
    .unwrap();

    let resolved = compose_with_precedence(vec![m1, m2]);

    // One conflict per shared field
    assert!(
        resolved.conflict_log.len() >= 2,
        "expected conflicts from shared asset and base type"
    );
    // Capabilities are unioned
    assert!(resolved.capabilities.contains(&BaseTypeCapability::Memory));
    assert!(
        resolved
            .capabilities
            .contains(&BaseTypeCapability::Evidence)
    );
}

#[test]
fn compose_with_precedence_no_conflicts_empty_log() {
    let m1 = IdentityManifest::new(
        "alpha",
        "1.0",
        vec![PromptAssetRef::new("asset-a", "a.md")],
        vec![BaseTypeId::new("bt-a")],
        capability_set([]),
        OperatingMode::Engineer,
        MemoryPolicy::default(),
        test_contract(),
    )
    .unwrap();
    let m2 = IdentityManifest::new(
        "beta",
        "1.0",
        vec![PromptAssetRef::new("asset-b", "b.md")],
        vec![BaseTypeId::new("bt-b")],
        capability_set([]),
        OperatingMode::Engineer,
        MemoryPolicy::default(),
        test_contract(),
    )
    .unwrap();

    let resolved = compose_with_precedence(vec![m1, m2]);
    assert!(resolved.conflict_log.is_empty());
    assert_eq!(resolved.prompt_assets.len(), 2);
    assert_eq!(resolved.base_types.len(), 2);
}

// ===========================================================================
// BuiltinIdentityLoader — catalog invariants
// ===========================================================================

#[test]
fn builtin_loader_composite_engineer_has_five_components() {
    let loader = BuiltinIdentityLoader;
    let manifest = loader
        .load(&IdentityLoadRequest::new(
            "simard-composite-engineer",
            "0.1.0",
            test_contract(),
        ))
        .unwrap();
    assert_eq!(
        manifest.components.len(),
        5,
        "composite engineer should have exactly 5 sub-identities"
    );
}

#[test]
fn builtin_loader_composite_engineer_component_names() {
    let loader = BuiltinIdentityLoader;
    let manifest = loader
        .load(&IdentityLoadRequest::new(
            "simard-composite-engineer",
            "0.1.0",
            test_contract(),
        ))
        .unwrap();
    let names: Vec<&str> = manifest.components.iter().map(|s| s.as_str()).collect();
    assert!(names.contains(&"simard-engineer"));
    assert!(names.contains(&"simard-meeting"));
    assert!(names.contains(&"simard-gym"));
    assert!(names.contains(&"simard-goal-curator"));
    assert!(names.contains(&"simard-improvement-curator"));
}

#[test]
fn builtin_loader_version_propagates_from_request() {
    let loader = BuiltinIdentityLoader;
    let manifest = loader
        .load(&IdentityLoadRequest::new(
            "simard-engineer",
            "99.88.77",
            test_contract(),
        ))
        .unwrap();
    assert_eq!(manifest.version, "99.88.77");
}

#[test]
fn builtin_loader_contract_propagates_from_request() {
    let contract = ManifestContract::new(
        "custom::ep",
        "x -> y",
        vec!["layer:custom".to_string()],
        Provenance::new("custom-src", "custom-loc"),
        Freshness::now().unwrap(),
    )
    .unwrap();
    let loader = BuiltinIdentityLoader;
    let manifest = loader
        .load(&IdentityLoadRequest::new(
            "simard-engineer",
            "1.0",
            contract,
        ))
        .unwrap();
    assert_eq!(manifest.contract.entrypoint, "custom::ep");
}

#[test]
fn builtin_loader_all_single_identities_have_capabilities() {
    let loader = BuiltinIdentityLoader;
    let names = [
        "simard-engineer",
        "simard-meeting",
        "simard-gym",
        "simard-goal-curator",
        "simard-improvement-curator",
    ];
    for name in names {
        let manifest = loader
            .load(&IdentityLoadRequest::new(name, "0.1.0", test_contract()))
            .unwrap();
        assert!(
            !manifest.required_capabilities.is_empty(),
            "{name} should have required capabilities"
        );
        assert!(
            manifest
                .required_capabilities
                .contains(&BaseTypeCapability::PromptAssets),
            "{name} should require PromptAssets"
        );
        assert!(
            manifest
                .required_capabilities
                .contains(&BaseTypeCapability::Memory),
            "{name} should require Memory"
        );
    }
}

#[test]
fn builtin_loader_engineer_has_expected_base_types() {
    let loader = BuiltinIdentityLoader;
    let manifest = loader
        .load(&IdentityLoadRequest::new(
            "simard-engineer",
            "0.1.0",
            test_contract(),
        ))
        .unwrap();
    let bt_names: Vec<&str> = manifest
        .supported_base_types
        .iter()
        .map(|bt| bt.as_str())
        .collect();
    assert!(bt_names.contains(&"local-harness"));
    assert!(bt_names.contains(&"terminal-shell"));
    assert!(bt_names.contains(&"rusty-clawd"));
    assert!(bt_names.contains(&"copilot-sdk"));
    assert!(bt_names.contains(&"claude-agent-sdk"));
    assert!(bt_names.contains(&"ms-agent-framework"));
}

#[test]
fn builtin_loader_engineer_has_terminal_shell_but_meeting_does_not() {
    let loader = BuiltinIdentityLoader;
    let engineer = loader
        .load(&IdentityLoadRequest::new(
            "simard-engineer",
            "0.1.0",
            test_contract(),
        ))
        .unwrap();
    let meeting = loader
        .load(&IdentityLoadRequest::new(
            "simard-meeting",
            "0.1.0",
            test_contract(),
        ))
        .unwrap();
    assert!(engineer.supports_base_type(&BaseTypeId::new("terminal-shell")));
    assert!(!meeting.supports_base_type(&BaseTypeId::new("terminal-shell")));
}

#[test]
fn builtin_loader_each_identity_has_unique_prompt_asset() {
    let loader = BuiltinIdentityLoader;
    let names = [
        "simard-engineer",
        "simard-meeting",
        "simard-gym",
        "simard-goal-curator",
        "simard-improvement-curator",
    ];
    let mut seen_asset_ids = BTreeSet::new();
    for name in names {
        let manifest = loader
            .load(&IdentityLoadRequest::new(name, "0.1.0", test_contract()))
            .unwrap();
        assert_eq!(
            manifest.prompt_assets.len(),
            1,
            "{name} should have exactly one prompt asset"
        );
        let asset_id = manifest.prompt_assets[0].id.as_str().to_string();
        assert!(
            seen_asset_ids.insert(asset_id.clone()),
            "{name} prompt asset id '{asset_id}' should be unique"
        );
    }
}

// ===========================================================================
// IdentityLoadRequest — construction
// ===========================================================================

#[test]
fn identity_load_request_stores_all_fields() {
    let contract = test_contract();
    let req = IdentityLoadRequest::new("my-id", "2.0.0", contract.clone());
    assert_eq!(req.identity, "my-id");
    assert_eq!(req.package_version, "2.0.0");
    assert_eq!(req.contract, contract);
}
