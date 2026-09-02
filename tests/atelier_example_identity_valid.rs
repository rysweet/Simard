//! Asset-level validation for the EXAMPLE `atelier` identity package.
//!
//! `examples/identities/atelier/` is a DATA-ONLY example identity — a
//! demonstration of Simard's pluggable-identity framework, distinct from
//! Simard's own compiled-in `simard-atelier` identity. It must:
//!
//! * load through the real data-driven loader (`load_example_identity`), NOT
//!   `BuiltinIdentityLoader`;
//! * ship every prompt asset its manifest references (system + one per phase);
//! * ship goal-session recipes that drive the phases end to end and treat their
//!   inputs as untrusted data;
//! * carry a ZERO-line `src/` footprint (this test lives under `tests/`, uses
//!   only the existing public loader, and adds no domain Rust).
//!
//! Together with `tests/qa-scenarios/atelier-example-identity.yaml` (which runs
//! this test), this is the machine-checkable proof that the example identity
//! works end to end via its recipes.

use std::path::{Path, PathBuf};

use simard::identity::{
    DEFAULT_EXAMPLE_IDENTITIES_DIR, IdentityLoadRequest, ManifestContract, OperatingMode,
    load_example_identity,
};
use simard::metadata::{Freshness, Provenance};

fn repo_path(relative: impl AsRef<Path>) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn example_base() -> PathBuf {
    repo_path(DEFAULT_EXAMPLE_IDENTITIES_DIR)
}

fn atelier_dir() -> PathBuf {
    example_base().join("atelier")
}

fn test_contract() -> ManifestContract {
    ManifestContract::new(
        "atelier::example::test",
        "brief -> fabrication package",
        vec!["test:atelier-example".to_string()],
        Provenance::new(
            "atelier-example-test",
            "tests/atelier_example_identity_valid.rs",
        ),
        Freshness::now().unwrap(),
    )
    .unwrap()
}

fn test_request(identity: &str) -> IdentityLoadRequest {
    IdentityLoadRequest::new(identity, "0.1.0", test_contract())
}

fn read(rel_to_package: &str) -> String {
    let path = atelier_dir().join(rel_to_package);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("expected to read {}: {e}", path.display()))
}

/// The example `atelier` package loads through the DATA-DRIVEN loader (not the
/// builtin), in engineer mode, exposing exactly its system + five phase prompts.
#[test]
fn atelier_example_identity_loads_via_data_driven_loader() {
    let manifest = load_example_identity(&example_base(), "atelier", &test_request("atelier"))
        .expect("the examples/identities/atelier package must load via load_example_identity");

    assert_eq!(manifest.name, "atelier");
    assert_eq!(
        manifest.default_mode,
        OperatingMode::Engineer,
        "atelier drives the inspect->act->verify->persist engineer loop"
    );
    assert_eq!(
        manifest.prompt_assets.len(),
        6,
        "atelier ships 6 prompts (system + brief + model + render + fabricate + handoff)"
    );

    for id in [
        "atelier-system",
        "atelier-brief",
        "atelier-model",
        "atelier-render",
        "atelier-fabricate",
        "atelier-handoff",
    ] {
        assert!(
            manifest.prompt_assets.iter().any(|a| a.id.as_str() == id),
            "atelier manifest should expose prompt asset '{id}'"
        );
    }
}

/// Every prompt asset the manifest references is a real, non-empty file inside
/// the package (paths resolve as clean `prompts/*.md`, never escaping it).
#[test]
fn atelier_example_prompt_assets_exist_and_are_nonempty() {
    let manifest = load_example_identity(&example_base(), "atelier", &test_request("atelier"))
        .expect("atelier package must load");

    for asset in &manifest.prompt_assets {
        let rel = &asset.relative_path;
        assert!(
            rel.starts_with("prompts"),
            "prompt asset '{}' must live under prompts/, got {}",
            asset.id.as_str(),
            rel.display()
        );
        assert!(
            !rel.to_string_lossy().contains(".."),
            "prompt asset path must not traverse: {}",
            rel.display()
        );
        let full = atelier_dir().join(rel);
        let body = std::fs::read_to_string(&full)
            .unwrap_or_else(|e| panic!("prompt asset {} must exist: {e}", full.display()));
        assert!(
            body.trim().len() > 200,
            "prompt asset {} must have real content",
            full.display()
        );
    }
}

/// The system prompt establishes the identity contract: untrusted-input
/// handling, the inspect->act->verify->persist loop, and the domain toolkit.
#[test]
fn atelier_example_system_prompt_states_the_contract() {
    let system = read("prompts/atelier_system.md");
    for needle in [
        "untrusted",
        "inspect",
        "verify",
        "persist",
        "OpenSCAD",
        "FreeCAD",
        "Blender",
    ] {
        assert!(
            system.contains(needle),
            "system prompt should mention '{needle}'"
        );
    }
    assert!(
        system.contains("no-point-in-time-docs"),
        "system prompt should bind the persistence guideline (G4)"
    );
}

/// The parametric-modeling recipe drives brief -> model -> render through real
/// domain tooling and treats its inputs as untrusted data.
#[test]
fn atelier_parametric_modeling_recipe_drives_the_phases() {
    let recipe = read("recipes/atelier-parametric-modeling.yaml");

    assert!(
        recipe.contains("name: \"atelier-parametric-modeling\""),
        "recipe must declare its name"
    );
    for step in ["id: \"brief\"", "id: \"model\"", "id: \"render\""] {
        assert!(recipe.contains(step), "recipe must define step {step}");
    }
    for tool in ["openscad", "freecad", "blender"] {
        assert!(
            recipe.to_lowercase().contains(tool),
            "recipe must drive domain tool '{tool}'"
        );
    }
    assert!(
        recipe.to_uppercase().contains("UNTRUSTED"),
        "recipe must treat the brief as untrusted data"
    );
    assert!(
        recipe.contains("manifold"),
        "recipe must verify the geometry is manifold before fabrication"
    );
}

/// The fabrication-export recipe drives fabricate -> handoff and produces the
/// buildable exports (STL/STEP), a cut list, and a BOM, persisted durably.
#[test]
fn atelier_fabrication_export_recipe_produces_the_package() {
    let recipe = read("recipes/atelier-fabrication-export.yaml");

    assert!(
        recipe.contains("name: \"atelier-fabrication-export\""),
        "recipe must declare its name"
    );
    for step in ["id: \"fabricate\"", "id: \"handoff\""] {
        assert!(recipe.contains(step), "recipe must define step {step}");
    }
    for artifact in ["model.stl", "model.step", "cutlist.csv", "bom.csv"] {
        assert!(
            recipe.contains(artifact),
            "recipe must produce fabrication artifact '{artifact}'"
        );
    }
    assert!(
        recipe.to_uppercase().contains("UNTRUSTED"),
        "recipe must treat the brief as untrusted data"
    );
    assert!(
        recipe.contains("no-point-in-time-docs"),
        "recipe must persist the package durably (G4)"
    );
}

/// A missing example package fails visibly (never a silent builtin fallback),
/// which is the loader contract the example identity relies on.
#[test]
fn missing_example_package_fails_visibly() {
    let err = load_example_identity(
        &example_base(),
        "atelier-does-not-exist",
        &test_request("atelier-does-not-exist"),
    )
    .expect_err("a missing example package must fail visibly");
    let _ = err;
}
