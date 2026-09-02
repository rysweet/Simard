//! Content-pin tests for the Creative Ideas semantic dedup + consolidation
//! assets (issue #2925; Group C typed-record rework of epic #4719).
//!
//! These pin the contract the Rust shim depends on WITHOUT re-running the agent:
//! the recipe files exist, expose exactly the `-c` context variables the shim
//! renders, name the terminal `output`, and — post-rework — instruct the agent
//! to ACT by CALLING the gated `simard ooda record-idea-*` tool (threading the
//! typed-record seam vars) rather than printing a scraped JSON envelope. The
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

/// The per-candidate dedup recipe exposes exactly the reasoning context vars the
/// shim (`recipe_brain.rs::run_idea_dedup_recipe`) renders, and names its
/// terminal output.
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
            "dedup recipe must reference the shim reasoning context var {var}"
        );
    }
    assert!(
        body.contains("output: \"idea_dedup_result\""),
        "dedup recipe must name the terminal output"
    );
    assert!(
        body.contains("agent: \"default\""),
        "dedup recipe must be a single default-agent reasoning step"
    );
}

/// Post-rework: the dedup recipe instructs the agent to ACT by CALLING the gated
/// `simard ooda record-idea-dedup` tool, threading the typed-record seam vars,
/// and scrapes NOTHING from stdout.
#[test]
fn dedup_recipe_calls_the_tool_and_threads_record_vars() {
    let body = asset(DEDUP_RECIPE);
    assert!(
        body.contains("record-idea-dedup"),
        "dedup recipe must instruct the agent to CALL `simard ooda record-idea-dedup`"
    );
    for seam in [
        "--record-path \"{{record_path}}\"",
        "--goal-id \"{{goal_id}}\"",
        "--cycle-number {{cycle_number}}",
    ] {
        assert!(
            body.contains(seam),
            "dedup recipe must thread the typed-record seam arg `{seam}` into the tool call"
        );
    }
    // The closed enum + per-variant ownership the tool enforces is documented.
    for token in ["create_new", "skip", "enhance_existing", "--target-node-id"] {
        assert!(
            body.contains(token),
            "dedup recipe must document the choice/field token {token}"
        );
    }
    // The fail-closed contract is stated so operators editing the prompt keep it.
    assert!(
        body.to_lowercase().contains("fail"),
        "dedup recipe must document the fail-closed contract"
    );
    // The forbidden scrape pattern is gone: no printed JSON envelope, no
    // clean-result-channel scraping language.
    assert!(
        body.contains("Output: NONE scraped from stdout"),
        "dedup recipe must document that stdout is NOT scraped"
    );
    for forbidden in [
        "clean result channel",
        "parse_idea_dedup_decision",
        "\"target_node_id\":",
    ] {
        assert!(
            !body.contains(forbidden),
            "dedup recipe must not carry the pre-rework scrape token `{forbidden}`"
        );
    }
}

/// The canonical prompt `.md` and the inlined recipe agree on the reasoning
/// context vars and the (post-rework) tool-call contract.
#[test]
fn dedup_prompt_and_recipe_agree() {
    let prompt = asset(DEDUP_PROMPT);
    for token in [
        "{{candidate_idea}}",
        "{{candidate_rationale}}",
        "{{existing_shortlist}}",
        "record-idea-dedup",
        "create_new",
        "skip",
        "enhance_existing",
        "--target-node-id",
        "{{record_path}}",
        "{{goal_id}}",
        "{{cycle_number}}",
    ] {
        assert!(
            prompt.contains(token),
            "canonical dedup prompt must contain {token}"
        );
    }
    assert!(
        !prompt.contains("\"target_node_id\":"),
        "canonical dedup prompt must not carry the pre-rework scraped JSON envelope"
    );
}

/// The consolidation recipe exposes the whole-pool reasoning context var, names
/// its terminal output, and — post-rework — instructs the agent to write its
/// cluster array to `{{clusters_path}}` and CALL the gated
/// `simard ooda record-idea-consolidation` tool.
#[test]
fn consolidation_recipe_exposes_pool_var_output_and_clusters_envelope() {
    let body = asset(CONSOLIDATION_RECIPE);
    assert!(
        body.contains("{{existing_pool}}"),
        "consolidation recipe must reference the whole-pool context var"
    );
    assert!(
        body.contains("output: \"idea_consolidation_result\""),
        "consolidation recipe must name the terminal output"
    );
    assert!(
        body.contains("record-idea-consolidation"),
        "consolidation recipe must instruct the agent to CALL `simard ooda record-idea-consolidation`"
    );
    for seam in [
        "--clusters-path \"{{clusters_path}}\"",
        "--record-path \"{{record_path}}\"",
        "--goal-id \"{{goal_id}}\"",
        "--cycle-number {{cycle_number}}",
    ] {
        assert!(
            body.contains(seam),
            "consolidation recipe must thread the typed-record seam arg `{seam}` into the tool call"
        );
    }
    // The cluster shape the tool validates is still documented for the agent.
    for token in [
        "clusters",
        "canonical_id",
        "redundant_ids",
        "merged_rationale",
    ] {
        assert!(
            body.contains(token),
            "consolidation recipe must document the cluster shape token {token}"
        );
    }
    assert!(
        body.contains("Output: NONE scraped from stdout"),
        "consolidation recipe must document that stdout is NOT scraped"
    );
    assert!(
        !body.contains("parse_idea_consolidation"),
        "consolidation recipe must not carry the pre-rework scrape symbol"
    );
}
