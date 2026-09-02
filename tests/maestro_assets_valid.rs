//! Asset-level validation for the EXAMPLE `maestro` music composition &
//! production identity package.
//!
//! `maestro` is a DATA-ONLY example identity under
//! `examples/identities/maestro/` (identity.toml + prompts/ + recipes/). It is
//! loaded by the data-driven [`load_example_identity`] rail — NEVER compiled
//! into [`BuiltinIdentityLoader`] and adding ZERO Rust to `src/`. These tests
//! prove the shipped assets actually parse and load through the real Simard
//! loaders, and that the goal-session recipe carries the stage contract that
//! drives the score + audio pipeline.
//!
//! This test target lives under `tests/` (an integration-test target), not
//! under `src/`, so it does not change Simard's daemon source tree.

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
        "maestro::assets::test",
        "brief -> score + audio",
        vec!["test:maestro".to_string()],
        Provenance::new("maestro-assets-test", "tests/maestro_assets_valid.rs"),
        Freshness::now().unwrap(),
    )
    .unwrap()
}

fn test_request(identity: &str) -> IdentityLoadRequest {
    IdentityLoadRequest::new(identity, "0.1.0", test_contract())
}

/// The shipped `examples/identities/maestro/` package must load end-to-end
/// through the data-driven [`load_example_identity`] rail, in curator mode,
/// exposing all six phase prompts.
#[test]
fn shipped_maestro_example_identity_loads() {
    let manifest = load_example_identity(&example_base(), "maestro", &test_request("maestro"))
        .expect("the examples/identities/maestro package must load via load_example_identity");

    assert_eq!(manifest.name, "maestro");
    assert_eq!(manifest.default_mode, OperatingMode::Curator);
    assert_eq!(
        manifest.prompt_assets.len(),
        6,
        "maestro ships 6 phase prompts (system + compose + arrange + engrave + produce + deliver)"
    );

    for expected in [
        "maestro-system",
        "maestro-compose",
        "maestro-arrange",
        "maestro-engrave",
        "maestro-produce",
        "maestro-deliver",
    ] {
        assert!(
            manifest
                .prompt_assets
                .iter()
                .any(|a| a.id.as_str() == expected),
            "maestro identity must expose the `{expected}` prompt asset"
        );
    }
}

/// The example identity declares a read-only write posture (it engraves/renders
/// artifacts into an operator-provided output dir; it does not need project
/// write authority).
#[test]
fn shipped_maestro_identity_is_read_only() {
    let manifest = load_example_identity(&example_base(), "maestro", &test_request("maestro"))
        .expect("maestro package must load");
    assert_eq!(
        manifest.authority.posture,
        WritePosture::ReadOnly,
        "maestro declares `posture = \"read-only\"` in identity.toml"
    );
    assert!(
        !manifest.memory_policy.allow_project_writes,
        "maestro memory policy must not allow project writes"
    );
}

/// Prove the package is DATA-DRIVEN, not compiled in: the
/// [`BuiltinIdentityLoader`] must NOT know this example identity.
#[test]
fn maestro_is_not_a_builtin_identity() {
    let builtin_err = BuiltinIdentityLoader
        .load(&test_request("maestro"))
        .expect_err("maestro must NOT be loadable via the BuiltinIdentityLoader — it is data-only");
    let _ = builtin_err; // any error is fine; the point is it is NOT a builtin.
}

/// Each referenced prompt asset file must actually exist on disk under the
/// package `prompts/` directory and carry its stage heading.
#[test]
fn shipped_maestro_prompt_files_exist_and_are_staged() {
    let prompts = example_base().join("maestro/prompts");
    for (file, heading) in [
        ("maestro_system.md", "Maestro System Prompt"),
        ("maestro_compose.md", "Stage 1: Composition"),
        ("maestro_arrange.md", "Stage 2: Arrangement & orchestration"),
        ("maestro_engrave.md", "Stage 3: Engraving"),
        ("maestro_produce.md", "Stage 4: Production"),
        ("maestro_deliver.md", "Stage 5: Score & production brief"),
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

/// The goal-session recipe must exist and drive the five-stage score + audio
/// pipeline end-to-end, carrying the context vars the stages consume and the
/// external domain tooling the pipeline shells out to (LilyPond / MuseScore /
/// MIDI + FluidSynth / TiMidity++ / ffmpeg) — all of which lives in the recipe,
/// never in `src/`.
#[test]
fn shipped_maestro_recipe_drives_the_score_and_audio_pipeline() {
    let recipe_path = example_base().join("maestro/recipes/maestro-score-and-produce.yaml");
    let recipe = std::fs::read_to_string(&recipe_path)
        .unwrap_or_else(|e| panic!("recipe {} must be readable: {e}", recipe_path.display()));

    for required in [
        "name:",
        "description:",
        "steps:",
        "maestro-score-and-produce",
    ] {
        assert!(
            recipe.contains(required),
            "recipe must contain top-level `{required}`"
        );
    }

    // The five pipeline stages, in order.
    for step_id in [
        "id: \"compose\"",
        "id: \"arrange\"",
        "id: \"engrave\"",
        "id: \"produce\"",
        "id: \"write-score-notes\"",
    ] {
        assert!(
            recipe.contains(step_id),
            "recipe must define the `{step_id}` stage"
        );
    }

    // The context vars the stages consume.
    for var in [
        "brief_path",
        "instrumentation",
        "key",
        "tempo",
        "duration",
        "soundfont",
        "output_dir",
    ] {
        assert!(
            recipe.contains(var),
            "recipe must thread the `{var}` context var through the stages"
        );
    }

    // The domain tooling lives in the recipe (not in Simard's daemon):
    // engraving (LilyPond / MuseScore) and the DAW render pass (MIDI + synths).
    for tool in ["lilypond", "mscore", "fluidsynth", "timidity", "ffmpeg"] {
        assert!(
            recipe.contains(tool),
            "recipe must drive the external `{tool}` tooling from the agent session"
        );
    }
}

/// A misspelled example name must fail visibly (fail-closed), never silently
/// fall back to a built-in identity.
#[test]
fn missing_maestro_variant_fails_visibly() {
    let err = load_example_identity(&example_base(), "maestroo", &test_request("maestroo"))
        .expect_err("a non-existent example package must return a clear error, not fall back");
    // The rail returns a fail-visible IdentityTomlParseError with the resolved path.
    let msg = format!("{err:?}");
    assert!(
        msg.contains("maestroo"),
        "the error must name the missing example package, got: {msg}"
    );
}
