//! Asset-level validation for the shipped Concierge identity.
//!
//! These tests prove the Concierge *capability-bearing* assets are not just
//! present but actually parse and validate through the real Simard loaders:
//!
//! * the pluggable identity card loads via `FileIdentityLoader` and stays in
//!   parity with the `BuiltinIdentityLoader` definition;
//! * the goal-session capability policy parses via
//!   `CapabilityPolicy::from_toml_file` and holds the least-privilege shape;
//! * the three goal-session recipes carry the command contract that drives the
//!   `simard_operator_probe concierge-run` / `simard::concierge` surface.

use std::path::{Path, PathBuf};

use simard::identity::{
    BuiltinIdentityLoader, FileIdentityLoader, IdentityLoadRequest, IdentityLoader,
    ManifestContract, OperatingMode,
};
use simard::metadata::{Freshness, Provenance};
use simard::typed_ooda::CapabilityPolicy;

fn repo_path(relative: impl AsRef<Path>) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn test_contract() -> ManifestContract {
    ManifestContract::new(
        "concierge::assets::test",
        "brief -> concept + prototype",
        vec!["test:concierge".to_string()],
        Provenance::new("concierge-assets-test", "tests/concierge_assets_valid.rs"),
        Freshness::now().unwrap(),
    )
    .unwrap()
}

#[test]
fn shipped_concierge_identity_card_loads_and_is_orchestrator_mode() {
    let prompt_root = repo_path("prompt_assets");
    let identity_dir = prompt_root.join("simard/identities/concierge");
    assert!(
        identity_dir.join("identity.toml").exists(),
        "concierge identity card should be shipped at {}",
        identity_dir.display()
    );

    let loader = FileIdentityLoader::new(&identity_dir, &prompt_root);
    let request = IdentityLoadRequest::new("simard-concierge", "0.1.0", test_contract());
    let manifest = loader
        .load(&request)
        .expect("shipped concierge identity.toml should load");

    assert_eq!(manifest.name, "simard-concierge");
    assert_eq!(manifest.default_mode, OperatingMode::Orchestrator);
    assert!(
        manifest
            .prompt_assets
            .iter()
            .any(|a| a.id.as_str() == "concierge-system"),
        "concierge identity should expose its system prompt asset"
    );
    assert!(
        manifest.supports_base_type(&simard::base_types::BaseTypeId::new("local-harness")),
        "concierge identity should support the local-harness base type"
    );
    assert!(
        !manifest.seed_goals.is_empty(),
        "concierge identity card should carry hospitality seed goals"
    );
}

#[test]
fn shipped_concierge_card_matches_builtin_identity() {
    // The card is the pluggable equivalent of the compiled-in identity; the two
    // must agree on name and default mode so operators get the same behaviour
    // whether they select the builtin or ship the file-based card.
    let prompt_root = repo_path("prompt_assets");
    let identity_dir = prompt_root.join("simard/identities/concierge");

    let file_loader = FileIdentityLoader::new(&identity_dir, &prompt_root);
    let builtin_loader = BuiltinIdentityLoader;

    let from_file = file_loader
        .load(&IdentityLoadRequest::new(
            "simard-concierge",
            "0.1.0",
            test_contract(),
        ))
        .expect("file-based concierge identity should load");
    let from_builtin = builtin_loader
        .load(&IdentityLoadRequest::new(
            "simard-concierge",
            "0.1.0",
            test_contract(),
        ))
        .expect("builtin concierge identity should load");

    assert_eq!(from_file.name, from_builtin.name);
    assert_eq!(from_file.default_mode, from_builtin.default_mode);
    assert_eq!(
        from_file.prompt_assets[0].id.as_str(),
        from_builtin.prompt_assets[0].id.as_str(),
        "card and builtin should expose the same system prompt asset id"
    );
    assert_eq!(
        from_file.prompt_assets[0].relative_path, from_builtin.prompt_assets[0].relative_path,
        "card and builtin should resolve the same system prompt path"
    );
}

#[test]
fn shipped_concierge_capability_policy_parses_and_is_least_privilege() {
    let policy_path =
        repo_path("prompt_assets/simard/policies/concierge-goal-session-capabilities.toml");
    let policy = CapabilityPolicy::from_toml_file(&policy_path)
        .expect("concierge goal-session capability policy should parse and validate");

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
fn shipped_concierge_recipes_drive_the_command_surface() {
    // Every concierge recipe is a well-formed recipe-runner recipe grounded in
    // the runnable concierge surface.
    for relative in [
        "prompt_assets/simard/recipes/concierge-hotel-design.yaml",
        "prompt_assets/simard/recipes/concierge-software-scaffold.yaml",
        "prompt_assets/simard/recipes/concierge-end-to-end.yaml",
    ] {
        let recipe = std::fs::read_to_string(repo_path(relative))
            .unwrap_or_else(|e| panic!("recipe {relative} should be readable: {e}"));
        for required in ["name:", "description:", "steps:", "Concierge"] {
            assert!(
                recipe.contains(required),
                "recipe {relative} must contain `{required}`"
            );
        }
    }

    // The design recipe writes a structured concept; the scaffold and
    // end-to-end recipes prove the runnable prototype through the operator
    // probe surface.
    let hotel_design = std::fs::read_to_string(repo_path(
        "prompt_assets/simard/recipes/concierge-hotel-design.yaml",
    ))
    .unwrap();
    assert!(
        hotel_design.contains("{{brief}}") && hotel_design.contains("{{concept_output}}"),
        "hotel-design recipe must consume the brief and emit a concept file"
    );

    for relative in [
        "prompt_assets/simard/recipes/concierge-software-scaffold.yaml",
        "prompt_assets/simard/recipes/concierge-end-to-end.yaml",
    ] {
        let recipe = std::fs::read_to_string(repo_path(relative)).unwrap();
        assert!(
            recipe.contains("concierge-run"),
            "recipe {relative} must prove the prototype via the `concierge-run` operator probe"
        );
    }
}
