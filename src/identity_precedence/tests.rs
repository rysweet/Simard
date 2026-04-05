use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::base_types::{BaseTypeCapability, BaseTypeId, capability_set};
use crate::identity::{ManifestContract, MemoryPolicy, OperatingMode};
use crate::metadata::{Freshness, Provenance};
use crate::prompt_assets::{PromptAssetId, PromptAssetRef};

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

fn make_manifest(
    name: &str,
    assets: Vec<PromptAssetRef>,
    base_types: Vec<BaseTypeId>,
    capabilities: BTreeSet<BaseTypeCapability>,
) -> crate::identity::IdentityManifest {
    crate::identity::IdentityManifest::new(
        name,
        "0.1.0",
        assets,
        base_types,
        capabilities,
        OperatingMode::Engineer,
        MemoryPolicy::default(),
        test_contract(),
    )
    .unwrap()
}

// ---- Test 1: Single manifest resolves to itself ----
#[test]
fn single_manifest_resolves_to_itself() {
    let assets = vec![PromptAssetRef::new("core", "core.md")];
    let base_types = vec![BaseTypeId::new("local-harness")];
    let caps = capability_set([BaseTypeCapability::Memory]);
    let manifest = make_manifest("alpha", assets.clone(), base_types.clone(), caps.clone());

    let resolver = PrecedenceResolver::new(vec![manifest]);
    let resolved = resolver.resolve_all();

    assert_eq!(resolved.prompt_assets.len(), 1);
    assert_eq!(resolved.prompt_assets[0].id, PromptAssetId::new("core"));
    assert_eq!(resolved.base_types.len(), 1);
    assert_eq!(resolved.capabilities, caps);
    assert!(resolved.conflict_log.is_empty());
}

// ---- Test 2: Non-overlapping assets merge cleanly ----
#[test]
fn non_overlapping_assets_merge_cleanly() {
    let m1 = make_manifest(
        "alpha",
        vec![PromptAssetRef::new("core", "core.md")],
        vec![BaseTypeId::new("harness-a")],
        capability_set([BaseTypeCapability::Memory]),
    );
    let m2 = make_manifest(
        "beta",
        vec![PromptAssetRef::new("extra", "extra.md")],
        vec![BaseTypeId::new("harness-b")],
        capability_set([BaseTypeCapability::Evidence]),
    );

    let resolver = PrecedenceResolver::new(vec![m1, m2]);
    let resolved = resolver.resolve_all();

    assert_eq!(resolved.prompt_assets.len(), 2);
    assert_eq!(resolved.base_types.len(), 2);
    assert!(resolved.conflict_log.is_empty());
}

// ---- Test 3: Conflicting assets — higher precedence wins ----
#[test]
fn conflicting_assets_higher_precedence_wins() {
    let m1 = make_manifest(
        "high-priority",
        vec![PromptAssetRef::new("shared", "path/high.md")],
        vec![],
        capability_set([]),
    );
    let m2 = make_manifest(
        "low-priority",
        vec![PromptAssetRef::new("shared", "path/low.md")],
        vec![],
        capability_set([]),
    );

    let resolver = PrecedenceResolver::new(vec![m1, m2]);
    let (assets, log) = resolver.resolve_prompt_assets();

    assert_eq!(assets.len(), 1);
    assert_eq!(
        assets[0].relative_path,
        std::path::PathBuf::from("path/high.md")
    );
    assert_eq!(log.len(), 1);
    assert_eq!(log.entries[0].winner, "high-priority");
    assert_eq!(log.entries[0].loser, "low-priority");
}

// ---- Test 4: Capabilities are unioned and deduplicated ----
#[test]
fn capabilities_are_unioned_and_deduplicated() {
    let m1 = make_manifest(
        "alpha",
        vec![],
        vec![],
        capability_set([BaseTypeCapability::Memory, BaseTypeCapability::Evidence]),
    );
    let m2 = make_manifest(
        "beta",
        vec![],
        vec![],
        capability_set([BaseTypeCapability::Evidence, BaseTypeCapability::Reflection]),
    );

    let resolver = PrecedenceResolver::new(vec![m1, m2]);
    let caps = resolver.resolve_capabilities();

    assert_eq!(caps.len(), 3);
    assert!(caps.contains(&BaseTypeCapability::Memory));
    assert!(caps.contains(&BaseTypeCapability::Evidence));
    assert!(caps.contains(&BaseTypeCapability::Reflection));
}

// ---- Test 5: ConflictLog records overrides ----
#[test]
fn conflict_log_records_overrides() {
    let m1 = make_manifest(
        "primary",
        vec![PromptAssetRef::new("identity", "primary.md")],
        vec![BaseTypeId::new("adapter-x")],
        capability_set([]),
    );
    let m2 = make_manifest(
        "secondary",
        vec![PromptAssetRef::new("identity", "secondary.md")],
        vec![BaseTypeId::new("adapter-x")],
        capability_set([]),
    );

    let resolver = PrecedenceResolver::new(vec![m1, m2]);
    let resolved = resolver.resolve_all();

    assert_eq!(resolved.conflict_log.len(), 2);
    let asset_conflict = resolved
        .conflict_log
        .entries
        .iter()
        .find(|e| e.field == "prompt_asset")
        .expect("expected prompt_asset conflict");
    assert_eq!(asset_conflict.winner, "primary");
    assert_eq!(asset_conflict.loser, "secondary");

    let bt_conflict = resolved
        .conflict_log
        .entries
        .iter()
        .find(|e| e.field == "base_type")
        .expect("expected base_type conflict");
    assert_eq!(bt_conflict.winner, "primary");
    assert_eq!(bt_conflict.loser, "secondary");
}

// ---- Test 6: resolve_all produces complete ResolvedIdentity ----
#[test]
fn resolve_all_produces_complete_resolved_identity() {
    let m1 = make_manifest(
        "manifest-a",
        vec![PromptAssetRef::new("core", "core.md")],
        vec![BaseTypeId::new("harness")],
        capability_set([BaseTypeCapability::Memory]),
    );
    let m2 = make_manifest(
        "manifest-b",
        vec![PromptAssetRef::new("extra", "extra.md")],
        vec![BaseTypeId::new("copilot")],
        capability_set([BaseTypeCapability::Evidence]),
    );

    let meta = vec![
        BTreeMap::from([("env".to_string(), "prod".to_string())]),
        BTreeMap::from([("region".to_string(), "us-west".to_string())]),
    ];

    let resolver = PrecedenceResolver::new(vec![m1, m2]);
    let resolved = resolver.resolve_all_with_metadata(&meta);

    assert_eq!(resolved.prompt_assets.len(), 2);
    assert_eq!(resolved.base_types.len(), 2);
    assert_eq!(resolved.capabilities.len(), 2);
    assert_eq!(resolved.metadata.len(), 2);
    assert_eq!(resolved.metadata["env"], "prod");
    assert_eq!(resolved.metadata["region"], "us-west");
    assert!(resolved.conflict_log.is_empty());
}

// ---- Test 7: Empty manifests list edge case ----
#[test]
fn empty_manifests_returns_default() {
    let resolver = PrecedenceResolver::new(vec![]);
    let resolved = resolver.resolve_all();

    assert!(resolved.prompt_assets.is_empty());
    assert!(resolved.capabilities.is_empty());
    assert!(resolved.base_types.is_empty());
    assert!(resolved.metadata.is_empty());
    assert!(resolved.conflict_log.is_empty());
}

// ---- Test 8: Base types — higher precedence wins on name collision ----
#[test]
fn base_types_higher_precedence_wins() {
    let m1 = make_manifest(
        "winner",
        vec![],
        vec![BaseTypeId::new("shared-adapter")],
        capability_set([]),
    );
    let m2 = make_manifest(
        "loser",
        vec![],
        vec![BaseTypeId::new("shared-adapter")],
        capability_set([]),
    );

    let resolver = PrecedenceResolver::new(vec![m1, m2]);
    let (base_types, log) = resolver.resolve_base_types();

    assert_eq!(base_types.len(), 1);
    assert_eq!(log.len(), 1);
    assert_eq!(log.entries[0].field, "base_type");
    assert_eq!(log.entries[0].winner, "winner");
}

// ---- Test 9: Metadata merge — higher precedence wins on key collision ----
#[test]
fn metadata_higher_precedence_wins_on_key_collision() {
    let m1 = make_manifest("high", vec![], vec![], capability_set([]));
    let m2 = make_manifest("low", vec![], vec![], capability_set([]));

    let meta = vec![
        BTreeMap::from([("env".to_string(), "production".to_string())]),
        BTreeMap::from([
            ("env".to_string(), "staging".to_string()),
            ("tier".to_string(), "free".to_string()),
        ]),
    ];

    let resolver = PrecedenceResolver::new(vec![m1, m2]);
    let (merged, log) = resolver.resolve_metadata(&meta);

    assert_eq!(merged["env"], "production");
    assert_eq!(merged["tier"], "free");
    assert_eq!(log.len(), 1);
    assert_eq!(log.entries[0].key, "env");
    assert_eq!(log.entries[0].winner, "high");
}

// ---- Test 10: Three manifests — cascading precedence ----
#[test]
fn three_manifests_cascading_precedence() {
    let m1 = make_manifest(
        "top",
        vec![PromptAssetRef::new("shared", "top.md")],
        vec![BaseTypeId::new("shared-bt")],
        capability_set([BaseTypeCapability::Memory]),
    );
    let m2 = make_manifest(
        "middle",
        vec![PromptAssetRef::new("shared", "middle.md")],
        vec![BaseTypeId::new("shared-bt"), BaseTypeId::new("unique-bt")],
        capability_set([BaseTypeCapability::Evidence]),
    );
    let m3 = make_manifest(
        "bottom",
        vec![PromptAssetRef::new("shared", "bottom.md")],
        vec![BaseTypeId::new("shared-bt")],
        capability_set([BaseTypeCapability::Reflection]),
    );

    let resolver = PrecedenceResolver::new(vec![m1, m2, m3]);
    let resolved = resolver.resolve_all();

    // Asset: "top" wins
    assert_eq!(resolved.prompt_assets.len(), 1);
    assert_eq!(
        resolved.prompt_assets[0].relative_path,
        std::path::PathBuf::from("top.md")
    );

    // Base types: "shared-bt" from top wins; "unique-bt" from middle included
    assert_eq!(resolved.base_types.len(), 2);

    // Capabilities: all three unioned
    assert_eq!(resolved.capabilities.len(), 3);

    // Conflicts: 2 prompt_asset (middle + bottom lose) + 2 base_type (middle + bottom lose for shared-bt)
    assert_eq!(resolved.conflict_log.len(), 4);
}

// ---- Test 11: ConflictLog new and is_empty ----
#[test]
fn conflict_log_starts_empty() {
    let log = ConflictLog::new();
    assert!(log.is_empty());
    assert_eq!(log.len(), 0);
}

// ---- Test 12: ConflictLog record populates entries ----
#[test]
fn conflict_log_record_populates_entries() {
    let mut log = ConflictLog::new();
    log.record("prompt_asset", "core", "alpha", "beta");

    assert!(!log.is_empty());
    assert_eq!(log.len(), 1);
    assert_eq!(log.entries[0].field, "prompt_asset");
    assert_eq!(log.entries[0].key, "core");
    assert_eq!(log.entries[0].winner, "alpha");
    assert_eq!(log.entries[0].loser, "beta");
}
