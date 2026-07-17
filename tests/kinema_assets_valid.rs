//! Asset-level validation for the shipped Kinema identity.
//!
//! These tests prove the Kinema *capability-bearing* assets are not just
//! present but actually parse and validate through the real Simard loaders:
//!
//! * the pluggable identity card loads via `FileIdentityLoader`;
//! * the goal-session capability policy parses via
//!   `CapabilityPolicy::from_toml_file` and holds the least-privilege shape;
//! * the three goal-session recipes (storyboarding, rigging, rendering) carry
//!   the parser-free command contract that drives the `simard kinema` surface.

use std::path::{Path, PathBuf};

use simard::identity::{
    FileIdentityLoader, IdentityLoadRequest, IdentityLoader, ManifestContract, OperatingMode,
};
use simard::metadata::{Freshness, Provenance};
use simard::typed_ooda::CapabilityPolicy;

fn repo_path(relative: impl AsRef<Path>) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn test_contract() -> ManifestContract {
    ManifestContract::new(
        "kinema::assets::test",
        "brief -> rendered sequence",
        vec!["test:kinema".to_string()],
        Provenance::new("kinema-assets-test", "tests/kinema_assets_valid.rs"),
        Freshness::now().unwrap(),
    )
    .unwrap()
}

#[test]
fn shipped_kinema_identity_card_loads_and_is_engineer_mode() {
    let prompt_root = repo_path("prompt_assets");
    let identity_dir = prompt_root.join("simard/identities/kinema");
    assert!(
        identity_dir.join("identity.toml").exists(),
        "kinema identity card should be shipped at {}",
        identity_dir.display()
    );

    let loader = FileIdentityLoader::new(&identity_dir, &prompt_root);
    let request = IdentityLoadRequest::new("simard-kinema", "0.1.0", test_contract());
    let manifest = loader
        .load(&request)
        .expect("shipped kinema identity.toml should load");

    assert_eq!(manifest.name, "simard-kinema");
    assert_eq!(manifest.default_mode, OperatingMode::Engineer);
    assert!(
        manifest
            .prompt_assets
            .iter()
            .any(|a| a.id.as_str() == "kinema-system"),
        "kinema identity should expose its system prompt asset"
    );
}

#[test]
fn shipped_kinema_capability_policy_parses_and_is_least_privilege() {
    let policy_path =
        repo_path("prompt_assets/simard/policies/kinema-goal-session-capabilities.toml");
    let policy = CapabilityPolicy::from_toml_file(&policy_path)
        .expect("kinema goal-session capability policy should parse and validate");

    // A successful parse already enforces actor == goal-session-actor,
    // terminal_calls_per_cycle == 1, session binding, and >=1 repository.
    assert!(
        policy
            .allowed_repositories
            .iter()
            .any(|r| r.owner == "rysweet" && r.name == "Simard"),
        "policy should be scoped to the governed Simard repository"
    );
    assert!(
        policy.max_concurrent_engineers >= 1,
        "policy should permit at least one engineer"
    );
}

#[test]
fn shipped_kinema_recipes_drive_the_cli_command_surface() {
    for relative in [
        "prompt_assets/simard/recipes/kinema-storyboarding.yaml",
        "prompt_assets/simard/recipes/kinema-rigging.yaml",
        "prompt_assets/simard/recipes/kinema-rendering.yaml",
    ] {
        let recipe = std::fs::read_to_string(repo_path(relative))
            .unwrap_or_else(|e| panic!("recipe {relative} should be readable: {e}"));

        for required in ["name:", "description:", "steps:", "brief_path", "out_dir"] {
            assert!(
                recipe.contains(required),
                "recipe {relative} must contain `{required}`"
            );
        }
        assert!(
            recipe.contains("kinema build"),
            "recipe {relative} must invoke the `kinema build` CLI surface"
        );
    }
}
