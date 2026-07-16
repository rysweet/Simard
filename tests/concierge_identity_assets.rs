//! Asset + identity contract for the `simard-concierge` hospitality identity.
//!
//! The Concierge is a prompt/recipe-driven domain identity: the only compiled
//! change is its registration in `BuiltinIdentityLoader`, and everything else
//! lives under `prompt_assets/simard/concierge/`. These tests pin the durable
//! seam so a rename or a deleted asset fails the build rather than silently
//! breaking the identity at runtime:
//!
//!   1. the builtin loader resolves `simard-concierge` to an engineer-mode
//!      manifest pointing at the `concierge-system` prompt asset;
//!   2. the system prompt and all six phase prompts exist;
//!   3. the three recipes exist and reference the phase assets + the scaffold;
//!   4. the runnable reference prototype ships all of its files and seeds the
//!      same room-type / rate-plan identifiers the design phases name.

use std::fs;
use std::path::{Path, PathBuf};

use simard::{
    BuiltinIdentityLoader, Freshness, IdentityLoadRequest, IdentityLoader, ManifestContract,
    OperatingMode, Provenance,
};

fn repo_path(relative: impl AsRef<Path>) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn read_required(relative: &str) -> String {
    let path = repo_path(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("required concierge asset {}: {error}", path.display()))
}

fn test_contract() -> ManifestContract {
    ManifestContract::new(
        simard::bootstrap_entrypoint(),
        "bootstrap-config -> identity-loader -> runtime-ports -> local-runtime",
        vec!["tests:concierge".to_string()],
        Provenance::new("test", "concierge_identity_assets"),
        Freshness::now().expect("freshness should be observable"),
    )
    .expect("contract should be valid")
}

#[test]
fn builtin_loader_resolves_concierge_engineer_identity() {
    let manifest = BuiltinIdentityLoader
        .load(&IdentityLoadRequest::new(
            "simard-concierge",
            "0.1.0",
            test_contract(),
        ))
        .expect("simard-concierge must be a builtin identity");

    assert_eq!(manifest.name, "simard-concierge");
    assert_eq!(manifest.default_mode, OperatingMode::Engineer);
    assert!(
        manifest
            .prompt_assets
            .iter()
            .any(|asset| asset.id.as_str() == "concierge-system"
                && asset.relative_path == std::path::Path::new("simard/concierge_system.md")),
        "concierge must load simard/concierge_system.md"
    );
    for base in ["local-harness", "terminal-shell", "copilot-sdk"] {
        assert!(
            manifest.supports_base_type(&simard::BaseTypeId::new(base)),
            "concierge should support base type {base}"
        );
    }
}

#[test]
fn concierge_system_and_phase_prompts_exist() {
    let system = read_required("prompt_assets/simard/concierge_system.md");
    assert!(
        system.contains("Simard Concierge"),
        "system prompt must name the Concierge"
    );
    // The system prompt must reference every phase asset so the identity knows
    // its own phases.
    for phase in [
        "property_layout.md",
        "guest_experience.md",
        "brand_design.md",
        "reservations_pms.md",
        "housekeeping.md",
        "channel_management.md",
    ] {
        assert!(
            system.contains(phase),
            "concierge_system.md must reference the {phase} phase asset"
        );
        // ... and the asset itself must exist.
        let body = read_required(&format!("prompt_assets/simard/concierge/{phase}"));
        assert!(
            body.contains("untrusted data"),
            "{phase} must carry the untrusted-data prompt-injection guard"
        );
    }
}

#[test]
fn concierge_recipes_exist_and_wire_the_assets() {
    let concept = read_required("prompt_assets/simard/recipes/concierge-hotel-concept.yaml");
    assert!(concept.contains("name: \"concierge-hotel-concept\""));
    for asset in [
        "property_layout.md",
        "guest_experience.md",
        "brand_design.md",
    ] {
        assert!(
            concept.contains(asset),
            "concept recipe must drive the {asset} design phase"
        );
    }

    let scaffold = read_required("prompt_assets/simard/recipes/concierge-scaffold-pms.yaml");
    assert!(scaffold.contains("name: \"concierge-scaffold-pms\""));
    assert!(
        scaffold.contains("examples/concierge_reservations_pms.rs"),
        "scaffold recipe must start from the bundled reference prototype"
    );
    assert!(
        scaffold.contains("cargo run --example concierge_reservations_pms"),
        "scaffold recipe must verify the prototype runs its self-verifying demo"
    );

    let e2e = read_required("prompt_assets/simard/recipes/concierge-end-to-end.yaml");
    assert!(e2e.contains("name: \"concierge-end-to-end\""));
    assert!(
        e2e.contains("design-hotel-concept") && e2e.contains("scaffold-reservations-pms"),
        "end-to-end recipe must chain concept then prototype"
    );
}

#[test]
fn concierge_reference_prototype_is_complete_and_seeds_from_the_concept() {
    // The runnable reference prototype is a pure-Rust example (#3181: no Python
    // anywhere in the tree) wired into `cargo test` via Cargo.toml.
    let prototype = read_required("examples/concierge_reservations_pms.rs");
    for symbol in [
        "fn seed_hotel",
        "fn reserve",
        "fn check_in",
        "fn check_out",
        "fn housekeeping_board",
        "fn channel_snapshot",
        "fn run_end_to_end_demo",
    ] {
        assert!(
            prototype.contains(symbol),
            "reference prototype must expose {symbol}"
        );
    }

    // The seed carries the room-type + rate-plan identifiers the design phases
    // hand off to the software.
    for code in ["STD", "DLX", "STE", "BAR", "ADV", "PKG"] {
        assert!(
            prototype.contains(code),
            "prototype seed must define the {code} identifier"
        );
    }

    // The example must be wired into `cargo test` (test = true) so the invariant
    // tests run under the normal gate, not just as a manual demo.
    let cargo = read_required("Cargo.toml");
    assert!(
        cargo.contains("name = \"concierge_reservations_pms\"") && cargo.contains("test = true"),
        "Cargo.toml must wire the concierge example into cargo test"
    );
}
