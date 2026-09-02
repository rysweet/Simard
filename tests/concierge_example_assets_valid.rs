//! Asset-level validation for the EXAMPLE `concierge` hospitality identity.
//!
//! `examples/identities/concierge/` is a DATA-ONLY example package (an
//! `identity.toml` manifest plus `prompts/` and `recipes/`). It is loaded by the
//! data-driven `load_example_identity` rail, NOT compiled into
//! `BuiltinIdentityLoader`, and it adds ZERO Rust to `src/`. It is DISTINCT from
//! Simard's own built-in `simard-concierge` identity.
//!
//! These tests prove the shipped example assets are not merely present but
//! actually parse and validate through the real Simard loader, and that the
//! goal-session recipes carry the reservations/PMS/housekeeping/channel-management
//! contract that delivers the identity's behavior end-to-end.

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
        "concierge::example::assets::test",
        "brief -> hospitality package",
        vec!["test:concierge-example".to_string()],
        Provenance::new(
            "concierge-example-assets-test",
            "tests/concierge_example_assets_valid.rs",
        ),
        Freshness::now().unwrap(),
    )
    .unwrap()
}

fn test_request() -> IdentityLoadRequest {
    IdentityLoadRequest::new("concierge", "0.1.0", test_contract())
}

/// The data-driven loader parses `examples/identities/concierge/identity.toml`
/// and yields a curator, read-only, five-phase manifest — proving the identity
/// card is valid without any `BuiltinIdentityLoader` entry.
#[test]
fn concierge_example_identity_loads_via_data_driven_loader() {
    let manifest = load_example_identity(&example_base(), "concierge", &test_request())
        .expect("the examples/identities/concierge package must load via load_example_identity");

    assert_eq!(manifest.name, "concierge");
    assert_eq!(
        manifest.default_mode,
        OperatingMode::Curator,
        "the concierge example runs in curator mode"
    );
    assert_eq!(
        manifest.authority.posture,
        WritePosture::ReadOnly,
        "the concierge example is an observe-only, read-only identity"
    );
    assert!(
        !manifest.memory_policy.allow_project_writes,
        "the concierge example must not permit project writes"
    );

    let ids: Vec<&str> = manifest
        .prompt_assets
        .iter()
        .map(|a| a.id.as_str())
        .collect();
    assert_eq!(
        manifest.prompt_assets.len(),
        5,
        "concierge ships 5 phase prompts (system + intake + experience + operations + deliver), got {ids:?}"
    );
    for expected in [
        "concierge-system",
        "concierge-intake",
        "concierge-experience",
        "concierge-operations",
        "concierge-deliver",
    ] {
        assert!(
            ids.contains(&expected),
            "concierge identity must expose the `{expected}` prompt asset, got {ids:?}"
        );
    }
}

/// Every prompt asset referenced by the manifest resolves to a real file inside
/// the package directory (no dangling or escaping references).
#[test]
fn concierge_example_prompt_assets_exist_on_disk() {
    let package_dir = example_base().join("concierge");
    let manifest = load_example_identity(&example_base(), "concierge", &test_request())
        .expect("concierge example package must load");

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
        assert!(
            body.to_lowercase().contains("untrusted data")
                || body.to_lowercase().contains("data, not instructions"),
            "prompt `{}` must instruct the agent to treat inputs as untrusted data",
            asset.id.as_str()
        );
    }
}

/// The goal-session recipes carry the reservations/PMS/housekeeping/channel
/// workflow contract and the safety invariants that deliver the identity's
/// behavior end-to-end.
#[test]
fn concierge_example_recipes_drive_the_hospitality_workflows() {
    let recipes_dir = example_base().join("concierge/recipes");

    // Both shipped recipes share the recipe-runner shape and the untrusted-data
    // guard, and both must exercise the reservation lifecycle + no-double-booking
    // safety invariant.
    for relative in [
        "concierge-hospitality-package.yaml",
        "concierge-reservation-lifecycle.yaml",
    ] {
        let path = recipes_dir.join(relative);
        let recipe = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("recipe {} should be readable: {e}", path.display()));

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

        // The reservation lifecycle and the no-double-booking safety invariant
        // are the operational core the recipe must exercise.
        assert!(
            recipe.contains("check-in") && recipe.contains("check-out"),
            "recipe {relative} must run the check-in/check-out lifecycle"
        );
        assert!(
            recipe.to_lowercase().contains("double-booking"),
            "recipe {relative} must enforce the no-double-booking invariant"
        );
        assert!(
            recipe.to_lowercase().contains("housekeeping"),
            "recipe {relative} must cover the housekeeping workflow"
        );
    }

    // The full-package recipe additionally covers property layout, brand design,
    // and channel management across its four stages.
    let package_recipe =
        std::fs::read_to_string(recipes_dir.join("concierge-hospitality-package.yaml")).unwrap();
    for stage in [
        "intake",
        "design-experience",
        "specify-operations",
        "assemble-and-verify",
    ] {
        assert!(
            package_recipe.contains(stage),
            "package recipe must contain the `{stage}` stage"
        );
    }
    for domain in ["property program", "brand", "channel"] {
        assert!(
            package_recipe.to_lowercase().contains(domain),
            "package recipe must cover the `{domain}` domain"
        );
    }
}

/// Guardrail: the example package is DATA ONLY — it must contain no `.rs` files,
/// so it can never smuggle Rust into the build.
#[test]
fn concierge_example_package_is_data_only() {
    let package_dir = example_base().join("concierge");
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
