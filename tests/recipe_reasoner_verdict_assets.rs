//! Content-pin tests for the recipe-brain verdict/decision parse fix
//! (issue #2419 family: #2421 decide/orient, #2428/#2430/#2435/#2462/#2463
//! merge-judge, #2429 metric).
//!
//! TDD (Step 7 — write tests first): these are RED until the implementation
//! step adds the additive `{{escalation_note}}` placeholder to the
//! decide / orient / merge-judge recipes, mirroring the lifecycle recipe that
//! already exposes it (issue #2432). The escalation seam is what lets each of
//! these recipe-backed brains re-prompt (schema-repair → escalate) on a
//! verdict/decision parse-miss before falling back deterministically, instead
//! of silently defaulting on the first miss.
//!
//! The lifecycle recipe is asserted as a GREEN anchor so a regression that
//! strips the placeholder from the already-fixed recipe is also caught.

use std::fs;
use std::path::PathBuf;

fn recipe(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("prompt_assets/simard/recipes")
        .join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("recipe {} must be readable: {e}", path.display()))
}

/// Every recipe-backed brain that drives a confidence-gated escalation ladder
/// must expose the `{{escalation_note}}` placeholder so the brain can inject
/// the schema-repair / high-effort instruction on each rung. Empty on the base
/// attempt (byte-identical base behaviour); populated on escalation rungs.
const LADDER_RECIPES: &[&str] = &[
    "ooda-engineer-lifecycle.yaml", // GREEN anchor (already fixed, #2432)
    "ooda-decide.yaml",             // #2421 — RED until Step 8
    "ooda-orient.yaml",             // #2421 — RED until Step 8
    "merge-readiness-judge.yaml",   // #2428/#2430/#2435/#2462/#2463 — RED until Step 8
];

#[test]
fn ladder_recipes_expose_escalation_note_placeholder() {
    let mut missing = Vec::new();
    for name in LADDER_RECIPES {
        let body = recipe(name);
        if !body.contains("{{escalation_note}}") {
            missing.push(*name);
        }
    }
    assert!(
        missing.is_empty(),
        "these recipe-backed brains must expose the {{{{escalation_note}}}} \
         escalation seam (issue #2419 family) but do not: {missing:?}"
    );
}

#[test]
fn lifecycle_recipe_documents_empty_on_base_contract() {
    // GREEN anchor: the already-fixed lifecycle recipe pins the base contract.
    // Step 8 should document the same contract in the newly-wired recipes, but
    // we only hard-require the placeholder above to avoid over-pinning prose.
    let body = recipe("ooda-engineer-lifecycle.yaml");
    assert!(
        body.contains("{{escalation_note}}"),
        "lifecycle recipe must keep the escalation_note placeholder"
    );
    assert!(
        body.to_lowercase().contains("empty on the base attempt"),
        "lifecycle recipe must keep documenting the empty-on-base contract"
    );
}

#[test]
fn merge_readiness_recipe_documents_structured_verdict_contract() {
    // The merge-judge fix surfaces a STRUCTURED verdict; the recipe must keep
    // instructing the agent to emit the `{"verdict": ...}` JSON object so the
    // caller can parse `ready`/`not_ready`/`unclear` from the json envelope
    // rather than the text-mode banner (#2462/#2463).
    let body = recipe("merge-readiness-judge.yaml");
    assert!(
        body.contains("\"verdict\""),
        "merge recipe must document the structured verdict JSON contract"
    );
    for kw in ["ready", "not_ready", "unclear"] {
        assert!(
            body.contains(kw),
            "merge recipe must document the '{kw}' verdict value"
        );
    }
}
