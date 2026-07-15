//! Atelier identity — outside-in integration coverage.
//!
//! Proves the pluggable `simard-atelier` identity (industrial & furniture
//! design) is selectable through the public builtin loader and that its shipped
//! prompt asset + goal-session recipes exist on disk, so a session can be built
//! for it and it can drive the parametric-modeling / fabrication pipeline.

use std::path::PathBuf;

use simard::{
    BaseTypeId, BuiltinIdentityLoader, Freshness, IdentityLoadRequest, IdentityLoader,
    ManifestContract, OperatingMode, Provenance,
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn test_contract() -> ManifestContract {
    ManifestContract::new(
        "atelier::entrypoint",
        "brief -> model",
        vec!["identity:atelier".to_string()],
        Provenance::new("atelier-test", "atelier-locator"),
        Freshness::now().unwrap(),
    )
    .unwrap()
}

fn load_atelier() -> simard::IdentityManifest {
    BuiltinIdentityLoader
        .load(&IdentityLoadRequest::new(
            "simard-atelier",
            "0.1.0",
            test_contract(),
        ))
        .expect("simard-atelier should be a selectable builtin identity")
}

#[test]
fn atelier_is_selectable_with_atelier_mode() {
    let manifest = load_atelier();
    assert_eq!(manifest.name, "simard-atelier");
    assert_eq!(manifest.default_mode, OperatingMode::Atelier);
    assert_eq!(OperatingMode::Atelier.to_string(), "atelier");
    assert_eq!(
        "atelier".parse::<OperatingMode>().unwrap(),
        OperatingMode::Atelier
    );
}

#[test]
fn atelier_supports_a_shell_base_type_for_cad_tools() {
    let manifest = load_atelier();
    // Driving Blender/FreeCAD/OpenSCAD needs a shell-capable base type.
    assert!(
        manifest.supports_base_type(&BaseTypeId::new("terminal-shell")),
        "atelier must support terminal-shell to drive CAD tooling"
    );
    assert!(manifest.supports_base_type(&BaseTypeId::new("local-harness")));
}

#[test]
fn atelier_declares_a_dedicated_system_prompt_asset() {
    let manifest = load_atelier();
    assert_eq!(manifest.prompt_assets.len(), 1);
    let asset = &manifest.prompt_assets[0];
    assert_eq!(asset.id.as_str(), "atelier-system");
    assert_eq!(
        asset.relative_path.to_string_lossy(),
        "simard/atelier_system.md"
    );
}

fn assert_nonempty_file(rel: &str) {
    let path = repo_root().join(rel);
    assert!(path.is_file(), "missing shipped asset: {rel}");
    let bytes = std::fs::metadata(&path)
        .unwrap_or_else(|e| panic!("cannot stat {rel}: {e}"))
        .len();
    assert!(bytes > 0, "shipped asset is empty: {rel}");
}

#[test]
fn atelier_ships_prompt_and_goal_session_recipes() {
    // The system prompt the manifest points at must exist on disk.
    let manifest = load_atelier();
    let prompt_rel = format!(
        "prompt_assets/{}",
        manifest.prompt_assets[0].relative_path.to_string_lossy()
    );
    assert_nonempty_file(&prompt_rel);

    // Both goal-session recipes (parametric modeling + fabrication export).
    assert_nonempty_file("prompt_assets/simard/recipes/atelier-parametric-model.yaml");
    assert_nonempty_file("prompt_assets/simard/recipes/atelier-fabrication-export.yaml");

    // Pluggable identity card.
    assert_nonempty_file("examples/atelier/identity.toml");
}

#[test]
fn atelier_prompt_covers_the_end_to_end_pipeline() {
    let prompt =
        std::fs::read_to_string(repo_root().join("prompt_assets/simard/atelier_system.md"))
            .expect("atelier system prompt should be readable");
    // The identity must express the full brief -> exported model + render loop.
    for needle in [
        "Blender", "FreeCAD", "OpenSCAD", "STEP", "STL", "cut list", "BOM", "render", "inspect",
    ] {
        assert!(
            prompt.to_lowercase().contains(&needle.to_lowercase()),
            "atelier prompt should mention '{needle}'"
        );
    }
}

#[test]
fn atelier_recipes_declare_expected_top_level_shape() {
    for (rel, name) in [
        (
            "prompt_assets/simard/recipes/atelier-parametric-model.yaml",
            "atelier-parametric-model",
        ),
        (
            "prompt_assets/simard/recipes/atelier-fabrication-export.yaml",
            "atelier-fabrication-export",
        ),
    ] {
        let text = std::fs::read_to_string(repo_root().join(rel)).unwrap();
        assert!(
            text.contains(&format!("name: \"{name}\"")),
            "{rel} should declare name: \"{name}\""
        );
        assert!(text.contains("\nsteps:"), "{rel} must declare steps");
        assert!(
            text.contains("type: \"agent\""),
            "{rel} must declare an agent step"
        );
    }
}
