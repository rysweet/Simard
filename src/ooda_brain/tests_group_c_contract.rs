//! Source- and recipe-level guardrails for the Group C rework (epic #4719,
//! issue #2925), authored **tests-first**.
//!
//! These are executable acceptance checks for the operator directive: the
//! creative-ideas **semantic-dedup** and **consolidation** seams must reason AND
//! act through a typed `simard ooda record-idea-dedup` / `record-idea-consolidation`
//! tool + a fail-CLOSED reader, exactly like the #4734 per-goal-cycle, #4785
//! orient/decide, and #4906 admission seams. The forbidden pattern — "recipe
//! emits JSON → Rust scrapes stdout via `extract_and_parse_json` → Rust acts" —
//! must be gone from these two seams.
//!
//! Unlike the behavioural tests in [`super::tests_record_idea_dedup_consolidation`]
//! and [`crate::operator_cli::tests_record_idea_dedup_consolidation`], these read
//! the on-disk sources/recipes directly, so they pin the *shape* of the code and
//! prompts (the brief's acceptance greps), not just runtime behaviour. They fail
//! RED against the pre-rework tree (recipe_brain still owns
//! `parse_idea_dedup_decision` / `parse_idea_consolidation` +
//! `IdeaDedupEnvelope` / `IdeaConsolidationEnvelope`, and the recipes still print
//! a scraped JSON envelope) and turn GREEN once the rework lands.
//!
//! CRITICAL scope guard (matches the brief): the rework deletes ONLY the
//! creative-ideas-EXCLUSIVE scrape symbols. The SHARED `extract.rs` machinery
//! (`extract_and_parse_json` / `extract_json_payload`) is still used by Group D
//! (not yet converted) and MUST survive — asserted below.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_rel(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

const RECIPE_BRAIN: &str = "src/ooda_brain/recipe_brain.rs";
const MOD_RS: &str = "src/ooda_brain/mod.rs";
const OODA_CLI: &str = "src/operator_cli/ooda.rs";
const DEDUP_RECIPE: &str = "prompt_assets/simard/recipes/creative-idea-dedup.yaml";
const CONSOLIDATION_RECIPE: &str = "prompt_assets/simard/recipes/creative-ideas-consolidation.yaml";

// ---------------------------------------------------------------------------
// The new typed record surface exists in mod.rs.
// ---------------------------------------------------------------------------

#[test]
fn mod_exposes_the_creative_ideas_record_surface() {
    let src = read_rel(MOD_RS);
    for symbol in [
        "IDEA_DEDUP_SCHEMA",
        "IDEA_CONSOLIDATION_SCHEMA",
        "IdeaDedupDecisionRecord",
        "IdeaConsolidationRecord",
        "read_verified_idea_dedup",
        "read_verified_idea_consolidation",
        // The two shared chokepoints reused by writer AND reader.
        "fn from_choice_fields",
        "fn sanitized",
    ] {
        assert!(
            src.contains(symbol),
            "mod.rs must define `{symbol}` (the typed creative-ideas record surface + chokepoints)"
        );
    }
}

#[test]
fn schema_strings_are_the_pinned_v1_values() {
    let src = read_rel(MOD_RS);
    assert!(
        src.contains("\"simard.creative.idea_dedup.v1\""),
        "mod.rs must pin the dedup schema string simard.creative.idea_dedup.v1"
    );
    assert!(
        src.contains("\"simard.creative.idea_consolidation.v1\""),
        "mod.rs must pin the consolidation schema string simard.creative.idea_consolidation.v1"
    );
}

// ---------------------------------------------------------------------------
// The scrape machinery is deleted from the two seams.
// ---------------------------------------------------------------------------

#[test]
fn recipe_brain_has_no_creative_ideas_scrape_machinery() {
    let src = read_rel(RECIPE_BRAIN);
    // The creative-ideas-EXCLUSIVE scrape symbols the brief lists as
    // safe-to-delete once the seams read a typed record.
    for forbidden in [
        "parse_idea_dedup_decision",
        "parse_idea_consolidation",
        "IdeaDedupEnvelope",
        "IdeaConsolidationEnvelope",
        "invoke_idea_dedup_raw",
        "invoke_idea_consolidation_raw",
    ] {
        assert!(
            !src.contains(forbidden),
            "recipe_brain.rs must not contain creative-ideas scrape symbol `{forbidden}` after \
             the rework — the two seams read a typed record, they scrape NOTHING from stdout"
        );
    }
}

#[test]
fn creative_ideas_seams_no_longer_scrape_json_from_stdout() {
    let src = read_rel(RECIPE_BRAIN);
    // The two seams must not route through the shared JSON scraper any more.
    // `extract_and_parse_json` survives in the tree for Group D, but it must not
    // co-occur with either creative-ideas adapter tag in a stdout-scrape call.
    assert!(
        !src.contains("extract_recipe_decision_output(&output.stdout, IDEA_DEDUP_ADAPTER_TAG)"),
        "the semantic-dedup seam must not scrape recipe stdout"
    );
    assert!(
        !src.contains(
            "extract_recipe_decision_output(&output.stdout, IDEA_CONSOLIDATION_ADAPTER_TAG)"
        ),
        "the consolidation seam must not scrape recipe stdout"
    );
}

#[test]
fn creative_ideas_seams_read_the_typed_record() {
    let src = read_rel(RECIPE_BRAIN);
    for required in [
        "read_verified_idea_dedup",
        "read_verified_idea_consolidation",
        // The seams thread the record path + identity into a fresh temp dir and
        // bind cycle to the reasoner-record sentinel (mirrors the Group A/B seams).
        "REASONER_RECORD_CYCLE",
    ] {
        assert!(
            src.contains(required),
            "recipe_brain.rs must run the recipe then read the typed record via `{required}` \
             (modelled on `run_per_goal_cycle_recipe` + `read_verified_*`)"
        );
    }
}

// ---------------------------------------------------------------------------
// The shared extract.rs survives (Group D not yet converted).
// ---------------------------------------------------------------------------

#[test]
fn shared_recipe_output_extract_survives_group_c() {
    // Retention guard: `extract.rs` + `extract_and_parse_json` /
    // `extract_json_payload` still back Group D seams. Group C deletes only the
    // creative-ideas-exclusive symbols and does NOT delete extract.rs.
    let path = repo_root().join("src/recipe_output/extract.rs");
    assert!(
        path.is_file(),
        "src/recipe_output/extract.rs MUST survive Group C — still used by Group D seams"
    );
    let extract = read_rel("src/recipe_output/extract.rs");
    for retained in ["extract_json_payload", "extract_and_parse_json"] {
        assert!(
            extract.contains(retained),
            "extract.rs MUST retain shared helper `{retained}` — Group C deletes only the \
             creative-ideas-exclusive scrape symbols"
        );
    }
}

// ---------------------------------------------------------------------------
// The operator CLI dispatches the two new record arms.
// ---------------------------------------------------------------------------

#[test]
fn operator_cli_dispatches_the_record_idea_arms() {
    let src = read_rel(OODA_CLI);
    for arm in ["\"record-idea-dedup\"", "\"record-idea-consolidation\""] {
        assert!(
            src.contains(arm),
            "operator_cli/ooda.rs must route the `ooda {arm}` command"
        );
    }
}

// ---------------------------------------------------------------------------
// The recipes CALL the tool and print NO scraped JSON envelope.
// ---------------------------------------------------------------------------

#[test]
fn dedup_recipe_calls_the_tool_and_threads_record_vars() {
    let body = read_rel(DEDUP_RECIPE);
    assert!(
        body.contains("record-idea-dedup"),
        "the dedup recipe must instruct the agent to CALL `simard ooda record-idea-dedup`"
    );
    for var in ["{{record_path}}", "{{goal_id}}", "{{cycle_number}}"] {
        assert!(
            body.contains(var),
            "the dedup recipe must thread the record seam var {var} to the tool"
        );
    }
}

#[test]
fn consolidation_recipe_calls_the_tool_and_threads_record_vars() {
    let body = read_rel(CONSOLIDATION_RECIPE);
    assert!(
        body.contains("record-idea-consolidation"),
        "the consolidation recipe must instruct the agent to CALL `simard ooda record-idea-consolidation`"
    );
    for var in [
        "{{record_path}}",
        "{{goal_id}}",
        "{{cycle_number}}",
        "{{clusters_path}}",
    ] {
        assert!(
            body.contains(var),
            "the consolidation recipe must thread the record seam var {var} to the tool"
        );
    }
}
