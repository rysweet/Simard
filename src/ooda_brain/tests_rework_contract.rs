//! Source- and recipe-level guardrails for the Group A rework (issue #4785).
//!
//! These are **executable acceptance checks authored tests-first** for the
//! operator directive: the OODA **Orient** and **Decide** phases must reason AND
//! act through a typed `simard ooda record-orient|record-decide` tool + a
//! fail-CLOSED reader, exactly like the #4734 per-goal-cycle seam. The forbidden
//! pattern — "recipe emits JSON/decimal → Rust scrapes stdout → Rust acts" — must
//! be gone from the orient/decide seams.
//!
//! Unlike the behavioural tests in [`super::tests_record_orient_decide`], these
//! read the on-disk sources/recipes directly, so they pin the *shape* of the
//! code and prompts (the exact greps in the brief's acceptance list), not just
//! runtime behaviour. They fail RED against the pre-rework tree (recipe_brain
//! still owns `parse_orient_outcome` / `decide_judgment_from_variant`, and the
//! recipes still print a decimal / JSON envelope) and turn GREEN once the rework
//! lands.
//!
//! CRITICAL scope guard: the rework deletes ONLY the orient/decide-EXCLUSIVE
//! parse machinery. The SHARED engineer-lifecycle seam
//! (`run_brain_ladder`, `extract_decision_envelope`, `DecisionEnvelope`,
//! `LifecycleParseOutcome`, `record_verdict_parse_metric`, `finalize_ladder_result`)
//! and `src/recipe_output/extract.rs` MUST survive — this file asserts that too.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_rel(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// The new record module exists and mod.rs re-exports its surface.
// ---------------------------------------------------------------------------

#[test]
fn orient_decide_record_module_exists() {
    let path = repo_root().join("src/ooda_brain/orient_decide_record.rs");
    assert!(
        path.is_file(),
        "the rework must add the dedicated record module {} (records, schema \
         consts, the two chokepoints, and the two fail-CLOSED readers)",
        path.display()
    );
}

#[test]
fn mod_reexports_the_orient_decide_record_surface() {
    let src = read_rel("src/ooda_brain/mod.rs");
    assert!(
        src.contains("orient_decide_record"),
        "mod.rs must declare `mod orient_decide_record;`"
    );
    for symbol in [
        "DecideChoice",
        "DecideDecisionRecord",
        "DECIDE_SCHEMA",
        "OrientFields",
        "OrientDecisionRecord",
        "ORIENT_SCHEMA",
        "read_verified_decide",
        "read_verified_orient",
    ] {
        assert!(
            src.contains(symbol),
            "mod.rs must re-export `{symbol}` from the new record module"
        );
    }
}

// ---------------------------------------------------------------------------
// recipe_brain.rs — the orient/decide JSON/decimal-scrape layer is DELETED.
// ---------------------------------------------------------------------------

#[test]
fn recipe_brain_has_no_orient_decide_scrape_machinery() {
    let src = read_rel("src/ooda_brain/recipe_brain.rs");
    // These are the orient/decide-EXCLUSIVE parse symbols the brief lists as
    // safe-to-delete once their callers/tests are removed.
    for forbidden in [
        "parse_orient_outcome",
        "OrientEnvelope",
        "extract_orient_envelope",
        "orient_judgment_from_envelope",
        "deterministic_floor",
        "decide_judgment_from_variant",
        "default_advance_goal",
        "decide_decision_choice",
    ] {
        assert!(
            !src.contains(forbidden),
            "recipe_brain.rs must not contain orient/decide scrape symbol `{forbidden}` \
             after the rework — the Orient/Decide seams read a typed record, they scrape \
             NOTHING from stdout"
        );
    }
}

#[test]
fn recipe_brain_judge_seams_read_the_typed_record() {
    let src = read_rel("src/ooda_brain/recipe_brain.rs");
    for required in [
        "run_decide_recipe",
        "run_orient_recipe",
        "read_verified_decide",
        "read_verified_orient",
    ] {
        assert!(
            src.contains(required),
            "recipe_brain.rs must run the recipe then read the typed record via `{required}` \
             (modelled on `run_per_goal_cycle_recipe` + `read_verified`)"
        );
    }
}

#[test]
fn recipe_brain_retains_shared_lifecycle_machinery() {
    // HIGH-risk scope guard: the engineer-lifecycle seam (out of scope for Group
    // A) still depends on this machinery. Deleting it would break B/C/D.
    let src = read_rel("src/ooda_brain/recipe_brain.rs");
    for retained in [
        "run_brain_ladder",
        "finalize_ladder_result",
        "record_verdict_parse_metric",
        "extract_decision_envelope",
        "DecisionEnvelope",
        "LifecycleParseOutcome",
    ] {
        assert!(
            src.contains(retained),
            "recipe_brain.rs MUST retain shared lifecycle symbol `{retained}` — the rework \
             deletes ONLY orient/decide-exclusive parse machinery"
        );
    }
}

#[test]
fn shared_recipe_output_extract_is_not_deleted() {
    // `extract.rs` still backs lifecycle / per-goal / admission envelopes; it is
    // blocked from deletion until Groups B/C/D.
    let path = repo_root().join("src/recipe_output/extract.rs");
    assert!(
        path.is_file(),
        "src/recipe_output/extract.rs MUST survive Group A — it is still used by the \
         lifecycle/per-goal/admission seams"
    );
}

// ---------------------------------------------------------------------------
// operator_cli — the two writer subcommands are dispatched.
// ---------------------------------------------------------------------------

#[test]
fn operator_cli_dispatches_the_record_orient_and_record_decide_arms() {
    let src = read_rel("src/operator_cli/ooda.rs");
    assert!(
        src.contains("\"record-orient\""),
        "operator_cli must route the `ooda record-orient` command"
    );
    assert!(
        src.contains("\"record-decide\""),
        "operator_cli must route the `ooda record-decide` command"
    );
}

// ---------------------------------------------------------------------------
// recipes/ooda-decide.yaml — acts via the tool, prints NO JSON envelope.
// ---------------------------------------------------------------------------

#[test]
fn decide_recipe_calls_the_record_decide_tool() {
    let yaml = read_rel("prompt_assets/simard/recipes/ooda-decide.yaml");
    assert!(
        yaml.contains("ooda record-decide"),
        "ooda-decide.yaml must record its verdict by calling `simard ooda record-decide` once, \
         exactly like ooda-per-goal-cycle.yaml calls `record-decision`"
    );
    for flag in ["--record-path", "--goal-id", "--cycle-number"] {
        assert!(
            yaml.contains(flag),
            "ooda-decide.yaml's tool call must pass `{flag}` (binds the record to the live ctx)"
        );
    }
}

#[test]
fn decide_recipe_declares_no_stdout_scraping() {
    let yaml = read_rel("prompt_assets/simard/recipes/ooda-decide.yaml");
    let lower = yaml.to_lowercase();
    assert!(
        lower.contains("none scraped from stdout") || lower.contains("no json"),
        "ooda-decide.yaml must state plainly that NOTHING is scraped from stdout — its tool \
         call IS the effect"
    );
}

#[test]
fn decide_recipe_has_no_json_envelope_or_first_word_scrape() {
    let yaml = read_rel("prompt_assets/simard/recipes/ooda-decide.yaml");
    let lower = yaml.to_lowercase();
    assert!(
        !yaml.contains("\"decision\""),
        "ooda-decide.yaml must not instruct the agent to emit a `{{\"decision\": ...}}` envelope \
         for Rust to scrape (the forbidden emit→parse→act pattern)"
    );
    assert!(
        !lower.contains("first word") && !lower.contains("first whitespace"),
        "ooda-decide.yaml must not instruct a first-word/first-token stdout scrape fallback"
    );
    assert!(
        !yaml.contains("Return **only**"),
        "ooda-decide.yaml must not instruct the agent to `Return **only**` a JSON object"
    );
}

// ---------------------------------------------------------------------------
// recipes/ooda-orient.yaml — acts via the tool, prints NO decimal/JSON.
// ---------------------------------------------------------------------------

#[test]
fn orient_recipe_calls_the_record_orient_tool() {
    let yaml = read_rel("prompt_assets/simard/recipes/ooda-orient.yaml");
    assert!(
        yaml.contains("ooda record-orient"),
        "ooda-orient.yaml must record its judgment by calling `simard ooda record-orient` once"
    );
    for flag in [
        "--record-path",
        "--goal-id",
        "--cycle-number",
        "--base-urgency",
    ] {
        assert!(
            yaml.contains(flag),
            "ooda-orient.yaml's tool call must pass `{flag}` — `--base-urgency` is persisted so \
             the reader can re-check the no-escalation invariant self-consistently"
        );
    }
}

#[test]
fn orient_recipe_declares_no_stdout_scraping() {
    let yaml = read_rel("prompt_assets/simard/recipes/ooda-orient.yaml");
    let lower = yaml.to_lowercase();
    assert!(
        lower.contains("none scraped from stdout") || lower.contains("no json"),
        "ooda-orient.yaml must state plainly that NOTHING is scraped from stdout"
    );
}

#[test]
fn orient_recipe_has_no_decimal_or_first_token_scrape() {
    let yaml = read_rel("prompt_assets/simard/recipes/ooda-orient.yaml");
    let lower = yaml.to_lowercase();
    assert!(
        !lower.contains("bare decimal") && !lower.contains("decimal float"),
        "ooda-orient.yaml must not instruct the agent to emit a bare decimal for Rust to scrape"
    );
    assert!(
        !lower.contains("first token"),
        "ooda-orient.yaml must not instruct a first-token stdout scrape"
    );
    assert!(
        !yaml.contains("Return **only**"),
        "ooda-orient.yaml must not instruct the agent to `Return **only**` a value for scraping"
    );
}
