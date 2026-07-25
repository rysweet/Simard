//! Asset-level validation for the shipped EXAMPLE Gastronome identity package.
//!
//! Gastronome is a DATA-ONLY example identity under
//! `examples/identities/gastronome/` — the same package shape as the reference
//! `cartographer` package. It is loaded by the data-driven loader
//! (`load_example_identity`), NEVER compiled into `BuiltinIdentityLoader`, and
//! adds ZERO domain Rust to `src/`.
//!
//! These tests exercise the identity end-to-end from its shipped data: the
//! package loads through the real Simard loader, exposes the expected culinary
//! menu-design prompts, is genuinely data-driven (not a builtin), stays
//! read-only / project-write-free, carries no `.rs` domain code, and ships an
//! agentic recipe whose four stages drive the menu-design workflow (compose ->
//! nutrition & cost -> scale -> schedule) grounded in the identity's prompts.

use std::path::{Path, PathBuf};

use simard::identity::{
    BuiltinIdentityLoader, DEFAULT_EXAMPLE_IDENTITIES_DIR, IdentityLoadRequest, IdentityLoader,
    ManifestContract, OperatingMode, WritePosture, load_example_identity,
};
use simard::metadata::{Freshness, Provenance};

fn repo_path(relative: impl AsRef<Path>) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn examples_base() -> PathBuf {
    repo_path(DEFAULT_EXAMPLE_IDENTITIES_DIR)
}

fn test_contract() -> ManifestContract {
    ManifestContract::new(
        "gastronome::assets::test",
        "brief -> menu package",
        vec!["test:gastronome".to_string()],
        Provenance::new("gastronome-assets-test", "tests/gastronome_assets_valid.rs"),
        Freshness::now().unwrap(),
    )
    .unwrap()
}

fn test_request(identity: &str) -> IdentityLoadRequest {
    IdentityLoadRequest::new(identity, "0.1.0", test_contract())
}

/// The shipped example package loads through the real data-driven loader and
/// exposes exactly the five culinary prompts, in curator mode.
#[test]
fn shipped_gastronome_package_loads_with_its_menu_design_prompts() {
    let manifest =
        load_example_identity(&examples_base(), "gastronome", &test_request("gastronome")).expect(
            "the examples/identities/gastronome package must load via load_example_identity",
        );

    assert_eq!(manifest.name, "gastronome");
    assert_eq!(
        manifest.default_mode,
        OperatingMode::Curator,
        "gastronome is a non-engineering curator identity"
    );

    let asset_ids: Vec<&str> = manifest
        .prompt_assets
        .iter()
        .map(|a| a.id.as_str())
        .collect();
    for expected in [
        "gastronome-system",
        "gastronome-compose",
        "gastronome-analyze",
        "gastronome-scale",
        "gastronome-schedule",
    ] {
        assert!(
            asset_ids.contains(&expected),
            "gastronome identity must expose the `{expected}` prompt asset, got {asset_ids:?}"
        );
    }
    assert_eq!(
        manifest.prompt_assets.len(),
        5,
        "gastronome ships 5 prompts (system + compose + analyze + scale + schedule)"
    );

    // Prompt assets resolve as clean, contained `prompts/*.md` refs.
    for asset in &manifest.prompt_assets {
        let rel = asset.relative_path.to_string_lossy();
        assert!(
            rel.starts_with("prompts/") && rel.ends_with(".md"),
            "prompt asset {} must resolve as prompts/*.md, got {rel}",
            asset.id.as_str()
        );
        assert!(
            repo_path("examples/identities/gastronome")
                .join(&asset.relative_path)
                .is_file(),
            "prompt asset file for {} must exist on disk",
            asset.id.as_str()
        );
    }
}

/// The package is a read-only, project-write-free demonstration identity.
#[test]
fn shipped_gastronome_is_read_only_and_writes_no_project_memory() {
    let manifest =
        load_example_identity(&examples_base(), "gastronome", &test_request("gastronome"))
            .expect("gastronome package must load");

    assert_eq!(
        manifest.authority.posture,
        WritePosture::ReadOnly,
        "an example culinary identity must stay read-only"
    );
    assert!(
        !manifest.memory_policy.allow_project_writes,
        "example identity must not enable project-scoped memory writes"
    );
}

/// Prove this is DATA-DRIVEN, not compiled in: the BuiltinIdentityLoader must
/// NOT know the gastronome example identity (no `src/` loader arm exists).
#[test]
fn gastronome_is_not_a_builtin_identity() {
    let builtin = BuiltinIdentityLoader
        .load(&test_request("gastronome"))
        .expect_err("gastronome must NOT be loadable via BuiltinIdentityLoader — it is data-only");
    let _ = builtin; // any error is fine; the point is it is NOT a builtin arm.
}

/// The package is data only: it carries no `.rs` domain code (the whole point
/// of the example-identity boundary — zero Rust under the package).
#[test]
fn gastronome_package_contains_no_rust_source() {
    let pkg = repo_path("examples/identities/gastronome");
    let mut stack = vec![pkg.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                assert_ne!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("rs"),
                    "example package must be data-only; found Rust source {}",
                    path.display()
                );
            }
        }
    }
}

/// The agentic recipe drives the full menu-design workflow end to end: four
/// ordered stages that mirror the identity's prompts, the brief/constraint
/// context surface, the untrusted-data guardrail, and durable persistence.
#[test]
fn gastronome_recipe_drives_the_menu_design_workflow() {
    let relative = "examples/identities/gastronome/recipes/gastronome-menu.yaml";
    let recipe = std::fs::read_to_string(repo_path(relative))
        .unwrap_or_else(|e| panic!("recipe {relative} should be readable: {e}"));

    // Recipe skeleton.
    for required in ["name:", "description:", "steps:", "type: \"agent\""] {
        assert!(
            recipe.contains(required),
            "recipe {relative} must contain `{required}`"
        );
    }

    // The four ordered menu-design stages, in workflow order.
    let mut cursor = 0usize;
    for stage in ["compose", "analyze", "scale", "schedule"] {
        let needle = format!("id: \"{stage}\"");
        let at = recipe[cursor..]
            .find(&needle)
            .map(|off| cursor + off)
            .unwrap_or_else(|| {
                panic!("recipe {relative} must define stage `{stage}` after the previous stage")
            });
        cursor = at + needle.len();
    }

    // The brief + constraint context surface the identity works from.
    for var in [
        "brief",
        "headcount",
        "dietary",
        "budget",
        "service_time",
        "output_dir",
    ] {
        assert!(
            recipe.contains(&format!("{{{{{var}}}}}")),
            "recipe {relative} must reference the `{var}` context variable"
        );
    }

    // Domain rigor the identity is defined by: nutrition, cost, and scaling.
    let lower = recipe.to_ascii_lowercase();
    for domain_term in [
        "nutrition",
        "cost per cover",
        "scale factor",
        "prep schedule",
    ] {
        assert!(
            lower.contains(domain_term),
            "recipe {relative} must exercise the `{domain_term}` menu-design concern"
        );
    }

    // Security + persistence contract mirrored from the prompts.
    assert!(
        lower.contains("untrusted data"),
        "recipe {relative} must instruct the agent to treat the brief as untrusted data"
    );
    assert!(
        lower.contains("no-point-in-time-docs"),
        "recipe {relative} must persist findings as a durable menu package (G4)"
    );
}
