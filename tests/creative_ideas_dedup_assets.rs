//! Content-pin tests for the Creative Ideas semantic dedup + consolidation
//! assets (issue #2925).
//!
//! These pin the contract the Rust shim depends on WITHOUT re-running the agent:
//! the recipe files exist, expose exactly the `-c` context variables the shim
//! renders, name the terminal `output` the clean-result channel reads, and
//! carry the machine-readable decision envelope tokens the parser expects. The
//! prompt `.md` is the canonical source the recipe inlines, so both must agree.
//!
//! Mirrors `tests/recipe_brain_verdict_assets.rs`.

use std::fs;
use std::path::PathBuf;

fn asset(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("asset {} must be readable: {e}", path.display()))
}

const DEDUP_RECIPE: &str = "prompt_assets/simard/recipes/creative-idea-dedup.yaml";
const DEDUP_PROMPT: &str = "prompt_assets/simard/creative_idea_dedup.md";
const CONSOLIDATION_RECIPE: &str = "prompt_assets/simard/recipes/creative-ideas-consolidation.yaml";

/// The per-candidate dedup recipe exposes exactly the context vars the shim
/// (`recipe_brain.rs::invoke_idea_dedup_raw`) renders, and names the terminal
/// output the clean-result channel reads.
#[test]
fn dedup_recipe_exposes_shim_context_vars_and_output() {
    let body = asset(DEDUP_RECIPE);
    for var in [
        "{{candidate_idea}}",
        "{{candidate_rationale}}",
        "{{existing_shortlist}}",
    ] {
        assert!(
            body.contains(var),
            "dedup recipe must reference the shim context var {var}"
        );
    }
    assert!(
        body.contains("output: \"idea_dedup_result\""),
        "dedup recipe must name the terminal output the shim reads"
    );
    assert!(
        body.contains("agent: \"default\""),
        "dedup recipe must be a single default-agent reasoning step"
    );
}

/// The dedup recipe carries the machine-readable decision envelope the parser
/// (`parse_idea_dedup_decision`) maps: `choice` + `target_node_id`, with the
/// three known choice tokens.
#[test]
fn dedup_recipe_documents_decision_envelope_tokens() {
    let body = asset(DEDUP_RECIPE);
    for token in [
        "\"choice\"",
        "target_node_id",
        "create_new",
        "skip",
        "enhance_existing",
    ] {
        assert!(
            body.contains(token),
            "dedup recipe must document the decision token {token}"
        );
    }
    // The fail-closed contract is stated so operators editing the prompt keep it.
    assert!(
        body.to_lowercase().contains("fail"),
        "dedup recipe must document the fail-closed contract"
    );
}

/// The canonical prompt `.md` and the inlined recipe agree on the context vars
/// and the decision tokens (the recipe inlines the prompt body).
#[test]
fn dedup_prompt_and_recipe_agree() {
    let prompt = asset(DEDUP_PROMPT);
    for token in [
        "{{candidate_idea}}",
        "{{candidate_rationale}}",
        "{{existing_shortlist}}",
        "create_new",
        "skip",
        "enhance_existing",
        "target_node_id",
    ] {
        assert!(
            prompt.contains(token),
            "canonical dedup prompt must contain {token}"
        );
    }
}

/// The consolidation recipe exposes the whole-pool context var, names its
/// terminal output, and documents the clusters envelope the parser
/// (`parse_idea_consolidation`) maps.
#[test]
fn consolidation_recipe_exposes_pool_var_output_and_clusters_envelope() {
    let body = asset(CONSOLIDATION_RECIPE);
    assert!(
        body.contains("{{existing_pool}}"),
        "consolidation recipe must reference the whole-pool context var"
    );
    assert!(
        body.contains("output: \"idea_consolidation_result\""),
        "consolidation recipe must name the terminal output the shim reads"
    );
    for token in [
        "clusters",
        "canonical_id",
        "redundant_ids",
        "merged_rationale",
    ] {
        assert!(
            body.contains(token),
            "consolidation recipe must document the clusters envelope token {token}"
        );
    }
}
