//! Asset-level validation for the shipped Cartographer identity.
//!
//! These tests prove the Cartographer *capability-bearing* assets are not just
//! present but actually parse and validate through the real Simard loaders:
//!
//! * the pluggable identity card loads via `FileIdentityLoader`;
//! * the goal-session capability policy parses via
//!   `CapabilityPolicy::from_toml_file` and holds the least-privilege shape;
//! * the three goal-session recipes carry the parser-free command contract that
//!   drives the `simard cartographer` surface.

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
        "cartographer::assets::test",
        "study -> dashboard",
        vec!["test:cartographer".to_string()],
        Provenance::new(
            "cartographer-assets-test",
            "tests/cartographer_assets_valid.rs",
        ),
        Freshness::now().unwrap(),
    )
    .unwrap()
}

#[test]
fn shipped_cartographer_identity_card_loads_and_is_engineer_mode() {
    let prompt_root = repo_path("prompt_assets");
    let identity_dir = prompt_root.join("simard/identities/cartographer");
    assert!(
        identity_dir.join("identity.toml").exists(),
        "cartographer identity card should be shipped at {}",
        identity_dir.display()
    );

    let loader = FileIdentityLoader::new(&identity_dir, &prompt_root);
    let request = IdentityLoadRequest::new("simard-cartographer", "0.1.0", test_contract());
    let manifest = loader
        .load(&request)
        .expect("shipped cartographer identity.toml should load");

    assert_eq!(manifest.name, "simard-cartographer");
    assert_eq!(manifest.default_mode, OperatingMode::Engineer);
    assert!(
        manifest
            .prompt_assets
            .iter()
            .any(|a| a.id.as_str() == "cartographer-system"),
        "cartographer identity should expose its system prompt asset"
    );
}

#[test]
fn shipped_cartographer_capability_policy_parses_and_is_least_privilege() {
    let policy_path =
        repo_path("prompt_assets/simard/policies/cartographer-goal-session-capabilities.toml");
    let policy = CapabilityPolicy::from_toml_file(&policy_path)
        .expect("cartographer goal-session capability policy should parse and validate");

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
fn shipped_cartographer_recipes_drive_the_cli_command_surface() {
    for (relative, expected_command) in [
        (
            "prompt_assets/simard/recipes/cartographer-exploratory-analysis.yaml",
            "cartographer build",
        ),
        (
            "prompt_assets/simard/recipes/cartographer-visualization-design.yaml",
            "cartographer build",
        ),
        (
            "prompt_assets/simard/recipes/cartographer-dashboard-delivery.yaml",
            "cartographer serve",
        ),
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
            recipe.contains(expected_command),
            "recipe {relative} must invoke the `{expected_command}` CLI surface"
        );
    }
}
