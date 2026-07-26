//! Content-pin tests for the recipe-brain verdict/decision parse fix
//! (issue #2419 family: #2421 decide/orient, #2429 metric).
//!
//! The `{{escalation_note}}` placeholder is the schema-repair seam that lets a
//! recipe-backed brain re-prompt (schema-repair → escalate) on a verdict/
//! decision parse-miss before falling back deterministically, instead of
//! silently defaulting on the first miss.
//!
//! The lifecycle recipe is asserted as a GREEN anchor so a regression that
//! strips the placeholder from the already-fixed recipe is also caught.
//!
//! NOTE (issue #4721): the merge-readiness judge is DELIBERATELY absent from
//! the escalation ladder here. The escalation seam exists only to recover from
//! brittle JSON parse-misses; #4721 removed JSON parsing from the merge judge
//! entirely — the agent now RECORDS its verdict by calling the
//! `simard merge record-verdict` tool (no envelope to scrape, nothing to
//! schema-repair). A missing/ambiguous record fails the deterministic rail
//! closed on the first pass, so there is no parse-miss to escalate. The
//! merge-judge's new tool contract is pinned separately by
//! `merge_readiness_recipe_records_verdict_via_tool_not_json` below.

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
    "ooda-decide.yaml",             // #2421 — parses a JSON decision, still laddered
    "ooda-orient.yaml",             // #2421 — parses a JSON orientation, still laddered
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
fn merge_readiness_recipe_records_verdict_via_tool_not_json() {
    // #4721: the merge judge no longer emits a JSON verdict for Simard to
    // scrape. The recipe MUST instruct the agent to record its decision by
    // calling the `simard merge record-verdict` tool with `--verdict merge`
    // or `--verdict hold`, and MUST NOT reintroduce a JSON verdict envelope.
    let body = recipe("merge-readiness-judge.yaml");
    assert!(
        body.contains("simard merge record-verdict"),
        "merge recipe must instruct the agent to act via the \
         `simard merge record-verdict` tool"
    );
    for arg in ["--verdict merge", "--verdict hold"] {
        assert!(
            body.contains(arg),
            "merge recipe must document the '{arg}' tool verdict"
        );
    }
    // Guard against regressing to the forbidden emit-JSON-then-scrape pattern:
    // the recipe must not carry a quoted `"verdict"` JSON key.
    assert!(
        !body.contains("\"verdict\""),
        "merge recipe must NOT document a JSON verdict envelope (#4721 removed \
         the emit-JSON→parse antipattern; the tool call is the output)"
    );
}
