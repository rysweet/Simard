//! Content-pin tests for the Simard Cartographer identity assets.
//!
//! The Cartographer identity (data storytelling & dashboards) is a builtin
//! sibling identity plus a set of recipes and a system prompt under
//! `prompt_assets/`. These tests pin the contract WITHOUT running an agent:
//!
//! - the builtin loader advertises `simard-cartographer` with the expected
//!   mode, prompt asset, base types, and capabilities;
//! - the system prompt exists and states the four-phase, inspect->act->verify
//!   ->persist, zero-fabrication contract for all four visualization tools;
//! - each phase recipe (+ the end-to-end recipe) exists, is a single default
//!   agent step with a clean `output:` result channel, exposes its context
//!   vars, and carries the fail-honest / serve-then-claim guardrails the
//!   persona depends on.
//!
//! Mirrors `tests/creative_ideas_dedup_assets.rs`.

use std::fs;
use std::path::PathBuf;

use simard::{
    BaseTypeCapability, BaseTypeId, BuiltinIdentityLoader, Freshness, IdentityLoadRequest,
    IdentityLoader, ManifestContract, OperatingMode, Provenance,
};

fn asset(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("asset {} must be readable: {e}", path.display()))
}

fn test_contract() -> ManifestContract {
    ManifestContract::new(
        "test::entrypoint",
        "a -> b",
        vec!["key:value".to_string()],
        Provenance::new("test-source", "test-locator"),
        Freshness::now().unwrap(),
    )
    .unwrap()
}

const SYSTEM_PROMPT: &str = "prompt_assets/simard/cartographer_system.md";
const EXPLORE_RECIPE: &str = "prompt_assets/simard/recipes/cartographer-explore.yaml";
const VISUALIZE_RECIPE: &str = "prompt_assets/simard/recipes/cartographer-visualize.yaml";
const DELIVER_RECIPE: &str = "prompt_assets/simard/recipes/cartographer-deliver.yaml";
const NARRATIVE_RECIPE: &str = "prompt_assets/simard/recipes/cartographer-narrative.yaml";
const DASHBOARD_RECIPE: &str = "prompt_assets/simard/recipes/cartographer-dashboard.yaml";

const ALL_RECIPES: &[&str] = &[
    EXPLORE_RECIPE,
    VISUALIZE_RECIPE,
    DELIVER_RECIPE,
    NARRATIVE_RECIPE,
    DASHBOARD_RECIPE,
];

// ---------------------------------------------------------------------------
// Builtin identity contract
// ---------------------------------------------------------------------------

/// The builtin loader advertises the Cartographer identity with the expected
/// mode and prompt asset, and the prompt asset actually exists on disk.
#[test]
fn builtin_loader_advertises_cartographer_identity() {
    let loader = BuiltinIdentityLoader;
    let manifest = loader
        .load(&IdentityLoadRequest::new(
            "simard-cartographer",
            "0.1.0",
            test_contract(),
        ))
        .expect("builtin loader must know simard-cartographer");

    assert_eq!(manifest.name, "simard-cartographer");
    assert_eq!(manifest.default_mode, OperatingMode::Engineer);
    assert_eq!(
        manifest.prompt_assets.len(),
        1,
        "cartographer must have exactly one system prompt asset"
    );
    assert_eq!(manifest.prompt_assets[0].id.as_str(), "cartographer-system");
    assert_eq!(
        manifest.prompt_assets[0].relative_path,
        PathBuf::from("simard/cartographer_system.md")
    );

    // The referenced prompt asset must exist under prompt_assets/.
    let prompt_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("prompt_assets")
        .join(&manifest.prompt_assets[0].relative_path);
    assert!(
        prompt_path.is_file(),
        "cartographer prompt asset must exist at {}",
        prompt_path.display()
    );
}

/// The Cartographer serves apps and writes code, so it must accept the same
/// engineer-grade base types (including the local terminal-backed path) and
/// require the engineer-grade capabilities.
#[test]
fn cartographer_identity_supports_engineer_grade_backends_and_capabilities() {
    let loader = BuiltinIdentityLoader;
    let manifest = loader
        .load(&IdentityLoadRequest::new(
            "simard-cartographer",
            "0.1.0",
            test_contract(),
        ))
        .unwrap();

    for bt in [
        "local-harness",
        "terminal-shell",
        "rusty-clawd",
        "copilot-sdk",
        "claude-agent-sdk",
        "ms-agent-framework",
    ] {
        assert!(
            manifest.supports_base_type(&BaseTypeId::new(bt)),
            "cartographer should support base type {bt}"
        );
    }

    for cap in [
        BaseTypeCapability::PromptAssets,
        BaseTypeCapability::SessionLifecycle,
        BaseTypeCapability::Memory,
        BaseTypeCapability::Evidence,
        BaseTypeCapability::Reflection,
    ] {
        assert!(
            manifest.required_capabilities.contains(&cap),
            "cartographer should require capability {cap:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// System prompt contract
// ---------------------------------------------------------------------------

/// The system prompt states the operating discipline, the four phases, all four
/// visualization tools, and the zero-fabrication / serve-then-claim guardrails.
#[test]
fn cartographer_system_prompt_states_the_end_to_end_contract() {
    let prompt = asset(SYSTEM_PROMPT);

    // Operating discipline required by the product architecture note.
    for phase in ["inspect", "act", "verify", "persist"] {
        assert!(
            prompt.to_lowercase().contains(phase),
            "system prompt must state the '{phase}' discipline"
        );
    }

    // The four delivery tools named in the objective.
    for tool in ["Streamlit", "Plotly", "Observable", "D3"] {
        assert!(
            prompt.contains(tool),
            "system prompt must name the delivery tool {tool}"
        );
    }

    // The four phases.
    for phase in [
        "exploratory analysis",
        "visualization design",
        "app delivery",
        "narrative",
    ] {
        assert!(
            prompt.to_lowercase().contains(phase),
            "system prompt must describe the phase '{phase}'"
        );
    }

    // Zero-BS guardrails: no fabrication, serve-then-claim, definition of done.
    assert!(
        prompt.to_lowercase().contains("no fabrication")
            || prompt.to_lowercase().contains("no fabricat"),
        "system prompt must forbid fabrication"
    );
    assert!(
        prompt.to_lowercase().contains("served") && prompt.to_lowercase().contains("dashboard"),
        "system prompt must require a served dashboard"
    );
    assert!(
        prompt.to_lowercase().contains("definition of done"),
        "system prompt must state a definition of done"
    );
}

// ---------------------------------------------------------------------------
// Recipe contract (shared shape)
// ---------------------------------------------------------------------------

/// Every cartographer recipe is a single default-agent reasoning step that
/// names a clean terminal `output:` result channel and identifies as the
/// cartographer persona.
#[test]
fn every_cartographer_recipe_is_a_default_agent_step_with_clean_output() {
    for rel in ALL_RECIPES {
        let body = asset(rel);
        assert!(
            body.contains("agent: \"default\""),
            "{rel} must be a single default-agent step"
        );
        assert!(
            body.contains("type: \"agent\""),
            "{rel} must declare an agent step type"
        );
        assert!(
            body.contains("output: \"cartographer_"),
            "{rel} must name a clean cartographer_* terminal output channel"
        );
        assert!(
            body.to_lowercase().contains("cartographer"),
            "{rel} must identify as the cartographer persona"
        );
        // The clean-result-channel discipline: stdout is not the source of truth.
        assert!(
            body.to_lowercase().contains("stdout"),
            "{rel} must document that stdout is ignored (clean result channel)"
        );
    }
}

// ---------------------------------------------------------------------------
// Per-recipe context-var + guardrail contract
// ---------------------------------------------------------------------------

#[test]
fn explore_recipe_exposes_dataset_question_and_findings_output() {
    let body = asset(EXPLORE_RECIPE);
    for var in ["{{dataset_path}}", "{{question}}", "{{findings_output}}"] {
        assert!(body.contains(var), "explore recipe must reference {var}");
    }
    assert!(
        body.to_lowercase().contains("fabricat"),
        "explore recipe must forbid fabricating findings"
    );
    assert!(
        body.contains("cannot_answer"),
        "explore recipe must carry the fail-honest cannot_answer contract"
    );
}

#[test]
fn visualize_recipe_exposes_findings_and_spec_output_and_all_tools() {
    let body = asset(VISUALIZE_RECIPE);
    for var in ["{{findings_path}}", "{{question}}", "{{spec_output}}"] {
        assert!(body.contains(var), "visualize recipe must reference {var}");
    }
    for tool in ["streamlit", "plotly", "observable", "d3"] {
        assert!(
            body.contains(tool),
            "visualize recipe must offer the delivery tool {tool}"
        );
    }
    assert!(
        body.contains("recommended_tool"),
        "visualize recipe must recommend a delivery tool"
    );
}

#[test]
fn deliver_recipe_requires_a_verified_served_dashboard() {
    let body = asset(DELIVER_RECIPE);
    for var in [
        "{{dataset_path}}",
        "{{spec_path}}",
        "{{app_dir}}",
        "{{port}}",
        "{{delivery_output}}",
    ] {
        assert!(body.contains(var), "deliver recipe must reference {var}");
    }
    // Serve-then-claim: must verify the port responds before claiming delivery.
    assert!(
        body.contains("serve_verified"),
        "deliver recipe must carry the serve_verified honesty flag"
    );
    assert!(
        body.contains("curl"),
        "deliver recipe must verify the served port (curl)"
    );
    assert!(
        body.to_lowercase().contains("actually serve")
            || body.to_lowercase().contains("actually serves")
            || body.to_lowercase().contains("must be up"),
        "deliver recipe must require the app to actually serve"
    );
}

#[test]
fn narrative_recipe_answers_the_question_from_real_numbers() {
    let body = asset(NARRATIVE_RECIPE);
    for var in [
        "{{findings_path}}",
        "{{delivery_path}}",
        "{{question}}",
        "{{narrative_output}}",
    ] {
        assert!(body.contains(var), "narrative recipe must reference {var}");
    }
    assert!(
        body.contains("## Answer"),
        "narrative recipe must lead with the answer"
    );
    assert!(
        body.to_lowercase().contains("never invent")
            || body.to_lowercase().contains("never fabricate"),
        "narrative recipe must forbid inventing figures"
    );
}

#[test]
fn dashboard_recipe_runs_all_four_phases_end_to_end() {
    let body = asset(DASHBOARD_RECIPE);
    for var in [
        "{{dataset_path}}",
        "{{question}}",
        "{{workdir}}",
        "{{port}}",
        "{{result_output}}",
    ] {
        assert!(body.contains(var), "dashboard recipe must reference {var}");
    }
    for phase in ["Explore", "Visualize", "Deliver", "Narrate"] {
        assert!(
            body.contains(phase),
            "dashboard recipe must run the {phase} phase"
        );
    }
    // The definition of done gates on an actually-served dashboard.
    assert!(
        body.contains("serve_verified") && body.contains("\"done\""),
        "dashboard recipe must gate 'done' on a verified served dashboard"
    );
    assert!(
        body.contains("served_url"),
        "dashboard recipe must report the served dashboard URL"
    );
}
