//! Asset-level validation for the EXAMPLE `terra` virtual-worlds & game-level
//! identity package.
//!
//! `terra` is a DATA-ONLY example identity under `examples/identities/terra/`
//! (identity.toml + prompts/ + recipes/). It is loaded by the data-driven
//! [`load_example_identity`] rail — NEVER compiled into
//! [`BuiltinIdentityLoader`] and adding ZERO Rust to `src/`. These tests prove
//! the shipped assets actually parse and load through the real Simard loaders,
//! and that the goal-session recipe carries the stage contract that drives the
//! world-building pipeline.
//!
//! This test target lives under `tests/` (an integration-test target), not under
//! `src/`, so it does not change Simard's daemon source tree.

use std::path::{Path, PathBuf};

use simard::identity::{
    BuiltinIdentityLoader, DEFAULT_EXAMPLE_IDENTITIES_DIR, IdentityLoadRequest, IdentityLoader,
    ManifestContract, OperatingMode, WritePosture, load_example_identity,
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
        "terra::assets::test",
        "brief -> scene",
        vec!["test:terra".to_string()],
        Provenance::new("terra-assets-test", "tests/terra_assets_valid.rs"),
        Freshness::now().unwrap(),
    )
    .unwrap()
}

fn test_request(identity: &str) -> IdentityLoadRequest {
    IdentityLoadRequest::new(identity, "0.1.0", test_contract())
}

/// The shipped `examples/identities/terra/` package must load end-to-end through
/// the data-driven [`load_example_identity`] rail, in curator mode, exposing all
/// five phase prompts.
#[test]
fn shipped_terra_example_identity_loads() {
    let manifest = load_example_identity(&example_base(), "terra", &test_request("terra"))
        .expect("the examples/identities/terra package must load via load_example_identity");

    assert_eq!(manifest.name, "terra");
    assert_eq!(manifest.default_mode, OperatingMode::Curator);
    assert_eq!(
        manifest.prompt_assets.len(),
        5,
        "terra ships 5 phase prompts (system + worldplan + build + assemble + deliver)"
    );

    for expected in [
        "terra-system",
        "terra-worldplan",
        "terra-build",
        "terra-assemble",
        "terra-deliver",
    ] {
        assert!(
            manifest
                .prompt_assets
                .iter()
                .any(|a| a.id.as_str() == expected),
            "terra identity must expose the `{expected}` prompt asset"
        );
    }
}

/// The example identity declares a read-only write posture (it builds a scene
/// into an operator-provided output dir; it does not need project write
/// authority).
#[test]
fn shipped_terra_identity_is_read_only() {
    let manifest = load_example_identity(&example_base(), "terra", &test_request("terra"))
        .expect("terra package must load");
    assert_eq!(
        manifest.authority.posture,
        WritePosture::ReadOnly,
        "terra declares `posture = \"read-only\"` in identity.toml"
    );
    assert!(
        !manifest.memory_policy.allow_project_writes,
        "terra memory policy must not allow project writes"
    );
}

/// Prove the package is DATA-DRIVEN, not compiled in: the
/// [`BuiltinIdentityLoader`] must NOT know this example identity.
#[test]
fn terra_is_not_a_builtin_identity() {
    let builtin_err = BuiltinIdentityLoader
        .load(&test_request("terra"))
        .expect_err("terra must NOT be loadable via the BuiltinIdentityLoader — it is data-only");
    let _ = builtin_err; // any error is fine; the point is it is NOT a builtin.
}

/// Each referenced prompt asset file must actually exist on disk under the
/// package `prompts/` directory and carry its stage heading.
#[test]
fn shipped_terra_prompt_files_exist_and_are_staged() {
    let prompts = example_base().join("terra/prompts");
    for (file, heading) in [
        ("terra_system.md", "Terra System Prompt"),
        ("terra_worldplan.md", "Stage 1: World design & blockout"),
        ("terra_build.md", "Stage 2: Terrain & asset authoring"),
        ("terra_assemble.md", "Stage 3: Scene assembly & interaction"),
        ("terra_deliver.md", "Stage 4: World brief"),
    ] {
        let path = prompts.join(file);
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("prompt {} must be readable: {e}", path.display()));
        assert!(
            body.contains(heading),
            "prompt {file} must carry its stage heading `{heading}`"
        );
        assert!(
            body.to_ascii_lowercase().contains("untrusted data")
                || body.to_ascii_lowercase().contains("data, not instructions"),
            "prompt {file} must instruct the agent to treat inputs as untrusted data"
        );
    }
}

/// The goal-session recipe must exist and drive the four-stage world-building
/// pipeline end-to-end, carrying the context vars the stages consume and the
/// external domain tooling the pipeline shells out to (Godot / Blender / A-Frame)
/// — all of which lives in the recipe, never in `src/`.
#[test]
fn shipped_terra_recipe_drives_the_world_pipeline() {
    let recipe_path = example_base().join("terra/recipes/terra-world-build.yaml");
    let recipe = std::fs::read_to_string(&recipe_path)
        .unwrap_or_else(|e| panic!("recipe {} must be readable: {e}", recipe_path.display()));

    for required in ["name:", "description:", "steps:", "terra-world-build"] {
        assert!(
            recipe.contains(required),
            "recipe must contain top-level `{required}`"
        );
    }

    // The four pipeline stages, in order.
    for step_id in [
        "id: \"worldplan\"",
        "id: \"build\"",
        "id: \"assemble\"",
        "id: \"write-world-brief\"",
    ] {
        assert!(
            recipe.contains(step_id),
            "recipe must define the `{step_id}` stage"
        );
    }

    // The context vars the stages consume.
    for var in [
        "brief_path",
        "engine",
        "assets_dir",
        "output_dir",
        "world_scale",
    ] {
        assert!(
            recipe.contains(var),
            "recipe must thread the `{var}` context var through the stages"
        );
    }

    // The domain tooling lives in the recipe (not in Simard's daemon).
    for tool in ["godot", "blender", "aframe"] {
        assert!(
            recipe.contains(tool),
            "recipe must drive the external `{tool}` tooling from the agent session"
        );
    }

    // The scene must be verified as launchable and navigable — the whole point of
    // the identity ("a launchable, navigable 3D scene from a world brief").
    let lower = recipe.to_ascii_lowercase();
    assert!(
        lower.contains("navigable") && lower.contains("launch"),
        "recipe must require verifying the scene launches and is navigable"
    );
}

/// A misspelled example name must fail visibly (fail-closed), never silently fall
/// back to a built-in identity.
#[test]
fn missing_terra_variant_fails_visibly() {
    let err = load_example_identity(&example_base(), "terraa", &test_request("terraa"))
        .expect_err("a non-existent example package must return a clear error, not fall back");
    // The rail returns a fail-visible IdentityTomlParseError with the resolved path.
    let msg = format!("{err:?}");
    assert!(
        msg.contains("terraa"),
        "the error must name the missing example package, got: {msg}"
    );
}
