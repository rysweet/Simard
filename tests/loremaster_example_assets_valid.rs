//! Asset-level validation for the EXAMPLE `loremaster` tabletop-RPG identity.
//!
//! `examples/identities/loremaster/` is a DATA-ONLY example package (an
//! `identity.toml` manifest plus `prompts/` and `recipes/`). It is loaded by the
//! data-driven `load_example_identity` rail, NOT compiled into
//! `BuiltinIdentityLoader`, and it adds ZERO Rust to `src/`. There is no built-in
//! `simard-loremaster` identity — this is defined entirely by its data files.
//!
//! These tests prove the shipped example assets are not merely present but
//! actually parse and validate through the real Simard loader, and that the
//! goal-session recipes carry the campaign-design + run-a-session contract
//! (SRD-legal content, XP-budget-balanced encounters, an end-to-end session run
//! with initiative and a Foundry VTT module) that delivers the identity's
//! behavior end-to-end.

use std::path::{Path, PathBuf};

use simard::identity::{
    DEFAULT_EXAMPLE_IDENTITIES_DIR, IdentityLoadRequest, ManifestContract, OperatingMode,
    WritePosture, load_example_identity,
};
use simard::metadata::{Freshness, Provenance};

fn repo_path(relative: impl AsRef<Path>) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn example_base() -> PathBuf {
    repo_path(DEFAULT_EXAMPLE_IDENTITIES_DIR)
}

fn test_contract() -> ManifestContract {
    ManifestContract::new(
        "loremaster::example::assets::test",
        "brief -> playable campaign module",
        vec!["test:loremaster-example".to_string()],
        Provenance::new(
            "loremaster-example-assets-test",
            "tests/loremaster_example_assets_valid.rs",
        ),
        Freshness::now().unwrap(),
    )
    .unwrap()
}

fn test_request() -> IdentityLoadRequest {
    IdentityLoadRequest::new("loremaster", "0.1.0", test_contract())
}

/// The data-driven loader parses `examples/identities/loremaster/identity.toml`
/// and yields a curator, read-only, five-phase manifest — proving the identity
/// card is valid without any `BuiltinIdentityLoader` entry.
#[test]
fn loremaster_example_identity_loads_via_data_driven_loader() {
    let manifest = load_example_identity(&example_base(), "loremaster", &test_request())
        .expect("the examples/identities/loremaster package must load via load_example_identity");

    assert_eq!(manifest.name, "loremaster");
    assert_eq!(
        manifest.default_mode,
        OperatingMode::Curator,
        "the loremaster example runs in curator mode"
    );
    assert_eq!(
        manifest.authority.posture,
        WritePosture::ReadOnly,
        "the loremaster example is an observe-only, read-only identity"
    );
    assert!(
        !manifest.memory_policy.allow_project_writes,
        "the loremaster example must not permit project writes"
    );

    let ids: Vec<&str> = manifest
        .prompt_assets
        .iter()
        .map(|a| a.id.as_str())
        .collect();
    assert_eq!(
        manifest.prompt_assets.len(),
        5,
        "loremaster ships 5 phase prompts (system + lore + encounters + prep + deliver), got {ids:?}"
    );
    for expected in [
        "loremaster-system",
        "loremaster-lore",
        "loremaster-encounters",
        "loremaster-prep",
        "loremaster-deliver",
    ] {
        assert!(
            ids.contains(&expected),
            "loremaster identity must expose the `{expected}` prompt asset, got {ids:?}"
        );
    }
}

/// Every prompt asset referenced by the manifest resolves to a real file inside
/// the package directory (no dangling or escaping references) and instructs the
/// agent to treat inputs as untrusted data.
#[test]
fn loremaster_example_prompt_assets_exist_on_disk() {
    let package_dir = example_base().join("loremaster");
    let manifest = load_example_identity(&example_base(), "loremaster", &test_request())
        .expect("loremaster example package must load");

    for asset in &manifest.prompt_assets {
        let resolved = package_dir.join(&asset.relative_path);
        assert!(
            resolved.is_file(),
            "prompt asset `{}` should resolve to a real file at {}",
            asset.id.as_str(),
            resolved.display()
        );
        let body = std::fs::read_to_string(&resolved)
            .unwrap_or_else(|e| panic!("prompt {} should be readable: {e}", resolved.display()));
        let lower = body.to_lowercase();
        assert!(
            lower.contains("untrusted data") || lower.contains("data, not instructions"),
            "prompt `{}` must instruct the agent to treat inputs as untrusted data",
            asset.id.as_str()
        );
        // Open-license discipline is a hard requirement for this identity.
        assert!(
            lower.contains("srd"),
            "prompt `{}` must constrain content to open SRD material",
            asset.id.as_str()
        );
    }
}

/// The goal-session recipes carry the campaign-design + run-a-session contract
/// and the safety invariants (SRD-legal, XP-budget balance, no accidental TPK)
/// that deliver the identity's behavior end-to-end.
#[test]
fn loremaster_example_recipes_drive_the_campaign_workflows() {
    let recipes_dir = example_base().join("loremaster/recipes");

    // Both shipped recipes share the recipe-runner shape and the untrusted-data
    // guard, and both must run a combat encounter (initiative -> resolution with
    // seeded dice) and enforce the SRD-legal + XP-budget + no-TPK invariants.
    for relative in [
        "loremaster-campaign-module.yaml",
        "loremaster-encounter-balance.yaml",
    ] {
        let path = recipes_dir.join(relative);
        let recipe = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("recipe {} should be readable: {e}", path.display()));
        let lower = recipe.to_lowercase();

        for required in ["name:", "description:", "steps:", "type: \"agent\""] {
            assert!(
                recipe.contains(required),
                "recipe {relative} must contain `{required}`"
            );
        }

        // Untrusted-data handling is a hard requirement for every recipe.
        assert!(
            recipe.contains("UNTRUSTED DATA"),
            "recipe {relative} must treat inputs as UNTRUSTED DATA"
        );

        // Running a session end-to-end is the operational core: initiative order,
        // seeded/reproducible dice, and a terminating encounter.
        assert!(
            lower.contains("initiative"),
            "recipe {relative} must roll initiative to run an encounter"
        );
        assert!(
            lower.contains("seed"),
            "recipe {relative} must use seeded/reproducible dice"
        );
        assert!(
            lower.contains("encounter"),
            "recipe {relative} must run an encounter"
        );

        // Open-license discipline + the XP-budget balance invariant.
        assert!(
            recipe.contains("SRD"),
            "recipe {relative} must constrain content to open SRD material"
        );
        assert!(
            lower.contains("xp budget"),
            "recipe {relative} must balance encounters to an XP budget"
        );
        assert!(
            lower.contains("multiplier"),
            "recipe {relative} must apply the SRD encounter multiplier when balancing"
        );
        // The no-accidental-TPK safety invariant.
        assert!(
            lower.contains("tpk") && lower.contains("deadly"),
            "recipe {relative} must enforce the no-accidental-TPK / deadly-threshold invariant"
        );
    }

    // The full-package recipe additionally covers world lore, session prep, and a
    // Foundry VTT module across its four stages.
    let package_recipe =
        std::fs::read_to_string(recipes_dir.join("loremaster-campaign-module.yaml")).unwrap();
    let package_lower = package_recipe.to_lowercase();
    for stage in [
        "world-and-lore",
        "npcs-and-encounters",
        "session-prep",
        "assemble-and-run",
    ] {
        assert!(
            package_recipe.contains(stage),
            "package recipe must contain the `{stage}` stage"
        );
    }
    for domain in ["lore", "npc", "encounter", "foundry"] {
        assert!(
            package_lower.contains(domain),
            "package recipe must cover the `{domain}` domain"
        );
    }
}

/// Guardrail: the example package is DATA ONLY — it must contain no `.rs` files,
/// so it can never smuggle Rust into the build.
#[test]
fn loremaster_example_package_is_data_only() {
    let package_dir = example_base().join("loremaster");
    let mut stack = vec![package_dir.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("reading {} should succeed: {e}", dir.display()))
        {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                assert_ne!(
                    path.extension().and_then(|s| s.to_str()),
                    Some("rs"),
                    "example package must be data-only; found a Rust file at {}",
                    path.display()
                );
            }
        }
    }
}
