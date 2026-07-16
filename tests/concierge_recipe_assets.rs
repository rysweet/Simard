//! Recipe-asset regression for the Concierge identity's shipped recipes.
//!
//! The Concierge identity ships three agentic recipes under
//! `prompt_assets/simard/recipes/`. This test reads them from the source tree
//! and asserts the structural invariants that keep them wired to the
//! deterministic backbone (`simard concierge …`) and to the file-channel output
//! contract (`{{*_output}}` / `{{*_report}}` written to a file, not stdout).
//!
//! Hermetic (reads files only), mirroring `recipe_context_file_assets.rs`.

use std::path::PathBuf;

fn recipe(filename: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("prompt_assets/simard/recipes")
        .join(filename);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("recipe {} must be readable: {e}", path.display()))
}

const RECIPES: &[&str] = &[
    "concierge-hotel-concept.yaml",
    "concierge-pms-scaffold.yaml",
    "concierge-end-to-end.yaml",
];

#[test]
fn every_concierge_recipe_declares_name_and_steps() {
    for name in RECIPES {
        let body = recipe(name);
        assert!(
            body.contains("name: \"concierge-"),
            "{name}: must declare a concierge recipe name"
        );
        assert!(body.contains("steps:"), "{name}: must declare steps");
        assert!(
            body.contains("type: \"agent\""),
            "{name}: must contain at least one agent step"
        );
        assert!(
            body.contains("author: \"Simard\""),
            "{name}: must be authored by Simard"
        );
    }
}

#[test]
fn every_concierge_recipe_stands_on_the_backbone() {
    // Each recipe must invoke the deterministic `simard concierge` backbone
    // rather than hand-rolling hospitality logic in the prompt.
    for name in RECIPES {
        let body = recipe(name);
        assert!(
            body.contains("simard concierge"),
            "{name}: must delegate to the `simard concierge` backbone"
        );
    }
}

#[test]
fn every_concierge_recipe_treats_brief_as_untrusted() {
    for name in RECIPES {
        let body = recipe(name);
        assert!(
            body.contains("untrusted"),
            "{name}: must treat the brief as untrusted data (XPIA hardening)"
        );
    }
}

#[test]
fn hotel_concept_recipe_covers_the_three_design_surfaces() {
    let body = recipe("concierge-hotel-concept.yaml");
    for surface in ["Property Layout", "Guest Experience", "Brand Design"] {
        assert!(
            body.contains(surface),
            "hotel-concept recipe must cover the '{surface}' design surface"
        );
    }
    // File-channel output contract.
    assert!(
        body.contains("{{concept_output}}"),
        "hotel-concept recipe must write to the concept_output file channel"
    );
}

#[test]
fn pms_scaffold_recipe_covers_the_four_services_and_verifies() {
    let body = recipe("concierge-pms-scaffold.yaml");
    for marker in [
        "BOOK",
        "CHECKIN",
        "CHECKOUT",
        "HOUSEKEEPING",
        "CHANNEL SYNC",
    ] {
        assert!(
            body.contains(marker),
            "pms-scaffold recipe must require the '{marker}' operation in the run trace"
        );
    }
    assert!(
        body.contains("simard concierge scaffold") && body.contains("simard concierge run"),
        "pms-scaffold recipe must both scaffold and run the prototype"
    );
    assert!(
        body.contains("{{scaffold_report}}"),
        "pms-scaffold recipe must write its verification report to a file channel"
    );
}

#[test]
fn end_to_end_recipe_delivers_both_concept_and_prototype() {
    let body = recipe("concierge-end-to-end.yaml");
    // Two-phase: design then deliver+verify.
    assert!(
        body.contains("phase 1 of 2") && body.contains("phase 2 of 2"),
        "end-to-end recipe must run design then delivery phases"
    );
    assert!(
        body.contains("simard concierge concept") && body.contains("simard concierge run"),
        "end-to-end recipe must design the concept AND run the prototype"
    );
    assert!(
        body.contains("{{delivery_report}}"),
        "end-to-end recipe must write a delivery report to a file channel"
    );
}
