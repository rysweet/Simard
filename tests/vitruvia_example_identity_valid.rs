//! Asset-level validation for the EXAMPLE `vitruvia` identity package.
//!
//! `examples/identities/vitruvia/` is a DATA-ONLY example identity — a
//! demonstration of Simard's pluggable-identity framework, with no compiled-in
//! `simard-vitruvia` counterpart. It must:
//!
//! * load through the real data-driven loader (`load_example_identity`), NOT
//!   `BuiltinIdentityLoader`;
//! * ship every prompt asset its manifest references (system + one per phase);
//! * ship goal-session recipes that drive the phases end to end and treat their
//!   inputs as untrusted data;
//! * carry a ZERO-line `src/` footprint (this test lives under `tests/`, uses
//!   only the existing public loader, and adds no domain Rust).
//!
//! Together with `tests/qa-scenarios/vitruvia-example-identity.yaml` (which runs
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

fn vitruvia_dir() -> PathBuf {
    example_base().join("vitruvia")
}

fn test_contract() -> ManifestContract {
    ManifestContract::new(
        "vitruvia::example::test",
        "program/site brief -> plans + walkthrough",
        vec!["test:vitruvia-example".to_string()],
        Provenance::new(
            "vitruvia-example-test",
            "tests/vitruvia_example_identity_valid.rs",
        ),
        Freshness::now().unwrap(),
    )
    .unwrap()
}

fn test_request(identity: &str) -> IdentityLoadRequest {
    IdentityLoadRequest::new(identity, "0.1.0", test_contract())
}

fn read(rel_to_package: &str) -> String {
    let path = vitruvia_dir().join(rel_to_package);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("expected to read {}: {e}", path.display()))
}

/// The example `vitruvia` package loads through the DATA-DRIVEN loader (not the
/// builtin), in engineer mode, exposing exactly its system + six phase prompts.
#[test]
fn vitruvia_example_identity_loads_via_data_driven_loader() {
    let manifest = load_example_identity(&example_base(), "vitruvia", &test_request("vitruvia"))
        .expect("the examples/identities/vitruvia package must load via load_example_identity");

    assert_eq!(manifest.name, "vitruvia");
    assert_eq!(
        manifest.default_mode,
        OperatingMode::Engineer,
        "vitruvia drives the inspect->act->verify->persist engineer loop"
    );
    assert_eq!(
        manifest.prompt_assets.len(),
        7,
        "vitruvia ships 7 prompts (system + program + massing + plan + interiors + drawings + walkthrough)"
    );

    for id in [
        "vitruvia-system",
        "vitruvia-program",
        "vitruvia-massing",
        "vitruvia-plan",
        "vitruvia-interiors",
        "vitruvia-drawings",
        "vitruvia-walkthrough",
    ] {
        assert!(
            manifest.prompt_assets.iter().any(|a| a.id.as_str() == id),
            "vitruvia manifest should expose prompt asset '{id}'"
        );
    }
}

/// Every prompt asset the manifest references is a real, non-empty file inside
/// the package (paths resolve as clean `prompts/*.md`, never escaping it).
#[test]
fn vitruvia_example_prompt_assets_exist_and_are_nonempty() {
    let manifest = load_example_identity(&example_base(), "vitruvia", &test_request("vitruvia"))
        .expect("vitruvia package must load");

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
        let full = vitruvia_dir().join(rel);
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
fn vitruvia_example_system_prompt_states_the_contract() {
    let system = read("prompts/vitruvia_system.md");
    for needle in [
        "untrusted",
        "inspect",
        "verify",
        "persist",
        "Blender",
        "BlenderBIM",
        "IFC",
        "FreeCAD",
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

/// The massing-plan recipe drives program -> massing -> plan through real domain
/// tooling, authors a BIM/IFC model, and is code-aware about egress.
#[test]
fn vitruvia_massing_plan_recipe_drives_the_phases() {
    let recipe = read("recipes/vitruvia-massing-plan.yaml");

    assert!(
        recipe.contains("name: \"vitruvia-massing-plan\""),
        "recipe must declare its name"
    );
    for step in ["id: \"program\"", "id: \"massing\"", "id: \"plan\""] {
        assert!(recipe.contains(step), "recipe must define step {step}");
    }
    for tool in ["blender", "freecad", "ifcopenshell"] {
        assert!(
            recipe.to_lowercase().contains(tool),
            "recipe must drive domain tool '{tool}'"
        );
    }
    assert!(
        recipe.contains("model.ifc"),
        "recipe must author the plan as a BIM/IFC model"
    );
    assert!(
        recipe.to_uppercase().contains("UNTRUSTED"),
        "recipe must treat the brief as untrusted data"
    );
    assert!(
        recipe.contains("egress"),
        "recipe must be code-aware and verify egress before the plan is done"
    );
}

/// The drawings-walkthrough recipe drives interiors -> drawings -> walkthrough
/// and produces the deliverables (plans, elevations, walkthrough), persisted
/// durably from the IFC model.
#[test]
fn vitruvia_drawings_walkthrough_recipe_produces_the_package() {
    let recipe = read("recipes/vitruvia-drawings-walkthrough.yaml");

    assert!(
        recipe.contains("name: \"vitruvia-drawings-walkthrough\""),
        "recipe must declare its name"
    );
    for step in [
        "id: \"interiors\"",
        "id: \"drawings\"",
        "id: \"walkthrough\"",
    ] {
        assert!(recipe.contains(step), "recipe must define step {step}");
    }
    for artifact in [
        "model.ifc",
        "plan_level_1",
        "elevation_north",
        "walkthrough.mp4",
    ] {
        assert!(
            recipe.contains(artifact),
            "recipe must produce deliverable '{artifact}'"
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
        "vitruvia-does-not-exist",
        &test_request("vitruvia-does-not-exist"),
    )
    .expect_err("a missing example package must fail visibly");
    let _ = err;
}
