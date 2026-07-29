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
//! parse machinery. The SHARED ladder backbone
//! (`run_brain_ladder`, `LifecycleParseOutcome`, `record_verdict_parse_metric`,
//! `finalize_ladder_result`) and `src/recipe_output/extract.rs` MUST survive —
//! this file asserts that too. (Group E #4967 later retired the lifecycle
//! stdout-scrape envelope itself; the Group E section at the end of this file
//! pins that seam onto the typed record.)

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
    // Scope guard: the shared OODA ladder backbone (used by every brain seam)
    // still exists. Group A deletes only orient/decide-exclusive parse
    // machinery; Group E (#4967) later retired the lifecycle stdout-scrape
    // envelope but PRESERVED this backbone (see the Group E section below).
    let src = read_rel("src/ooda_brain/recipe_brain.rs");
    for retained in [
        "run_brain_ladder",
        "finalize_ladder_result",
        "record_verdict_parse_metric",
        "LifecycleParseOutcome",
    ] {
        assert!(
            src.contains(retained),
            "recipe_brain.rs MUST retain shared ladder symbol `{retained}` — the rework \
             deletes ONLY seam-exclusive parse machinery, not the shared backbone"
        );
    }
}

#[test]
fn shared_recipe_output_extract_is_not_deleted() {
    // `extract.rs` still backs per-goal / admission envelopes and the
    // out-of-scope text consumers (journal `pr_source`, goal-curation
    // `recipe_progress_checker`); Group E (#4967) retired the lifecycle use.
    let path = repo_root().join("src/recipe_output/extract.rs");
    assert!(
        path.is_file(),
        "src/recipe_output/extract.rs MUST survive Group A — it is still used by the \
         per-goal/admission seams and the out-of-scope text consumers"
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

// ===========================================================================
// GROUP B — engineer- + resource-ADMISSION rework (issue #4906).
//
// The two admission seams must reason AND act through a typed
// `simard ooda record-admission|record-resource-admission` tool + a fail-CLOSED
// reader, exactly like Group A. These pin the *shape* of the post-rework sources
// and recipes (the brief's acceptance greps). They fail RED against the current
// tree (recipe_brain still owns `parse_admission_decision` /
// `parse_resource_admission_decision` + `AdmissionEnvelope` /
// `ResourceAdmissionEnvelope`, and the recipes still print a JSON envelope) and
// turn GREEN once the rework lands.
//
// CRITICAL scope guard (A7): the rework deletes ONLY the SIX admission-EXCLUSIVE
// scrape symbols. The SHARED `extract.rs` machinery
// (`extract_and_parse_json` / `extract_json_payload`) is still used by the
// per-goal / creative-ideas seams and the out-of-scope text consumers, and MUST
// survive — asserted below. (Group E #4967 later retired the lifecycle use.)
// ===========================================================================

#[test]
fn mod_reexports_the_admission_record_surface() {
    let src = read_rel("src/ooda_brain/mod.rs");
    for symbol in [
        "ADMISSION_SCHEMA",
        "RESOURCE_ADMISSION_SCHEMA",
        "AdmissionDecisionRecord",
        "ResourceAdmissionDecisionRecord",
        "read_verified_admission",
        "read_verified_resource_admission",
    ] {
        assert!(
            src.contains(symbol),
            "mod.rs must re-export `{symbol}` (the typed admission record surface)"
        );
    }
}

#[test]
fn recipe_brain_has_no_admission_scrape_machinery() {
    let src = read_rel("src/ooda_brain/recipe_brain.rs");
    // The SIX admission-EXCLUSIVE scrape symbols the brief lists as
    // safe-to-delete once the seams read a typed record.
    for forbidden in [
        "parse_admission_decision",
        "admission_decision_from_variant",
        "parse_resource_admission_decision",
        "resource_admission_decision_from_variant",
        "AdmissionEnvelope",
        "ResourceAdmissionEnvelope",
        "invoke_admission_raw",
        "invoke_resource_admission_raw",
    ] {
        assert!(
            !src.contains(forbidden),
            "recipe_brain.rs must not contain admission scrape symbol `{forbidden}` after the \
             rework — the admission seams read a typed record, they scrape NOTHING from stdout"
        );
    }
}

#[test]
fn recipe_brain_admission_seams_read_the_typed_record() {
    let src = read_rel("src/ooda_brain/recipe_brain.rs");
    for required in [
        "run_admission_recipe",
        "run_resource_admission_recipe",
        "read_verified_admission",
        "read_verified_resource_admission",
    ] {
        assert!(
            src.contains(required),
            "recipe_brain.rs must run the recipe then read the typed record via `{required}` \
             (modelled on `run_per_goal_cycle_recipe` + `read_verified`)"
        );
    }
}

#[test]
fn admission_seams_no_longer_scrape_json_from_stdout() {
    // The two admission seams must not route through the shared JSON scraper any
    // more. `extract_and_parse_json` survives in the tree for OTHER seams, but it
    // must appear NOWHERE in the admission-specific writer/reader code — which,
    // post-rework, no longer exists in recipe_brain as a scrape path. We assert
    // the admission adapter tags no longer co-occur with a stdout-scrape call by
    // requiring the deleted scrape helpers (above) to be gone; here we add the
    // direct guard that the admission recipe-output extractor is gone too.
    let src = read_rel("src/ooda_brain/recipe_brain.rs");
    assert!(
        !src.contains("extract_recipe_decision_output(&output.stdout, ADMISSION_ADAPTER_TAG)"),
        "the engineer-admission seam must not scrape recipe stdout"
    );
    assert!(
        !src.contains(
            "extract_recipe_decision_output(&output.stdout, RESOURCE_ADMISSION_ADAPTER_TAG)"
        ),
        "the resource-admission seam must not scrape recipe stdout"
    );
}

#[test]
fn shared_recipe_output_extract_survives_group_b() {
    // A7 retention guard: `extract.rs` + `extract_and_parse_json` /
    // `extract_json_payload` still back the per-goal / creative-ideas seams and
    // the out-of-scope text consumers. Group B deletes only the SIX
    // admission-exclusive symbols. (Group E #4967 later retired the lifecycle use.)
    let path = repo_root().join("src/recipe_output/extract.rs");
    assert!(
        path.is_file(),
        "src/recipe_output/extract.rs MUST survive Group B — still used by non-admission seams"
    );
    let extract = read_rel("src/recipe_output/extract.rs");
    for retained in ["extract_json_payload", "extract_and_parse_json"] {
        assert!(
            extract.contains(retained),
            "extract.rs MUST retain shared helper `{retained}` — Group B deletes only the six \
             admission-exclusive scrape symbols"
        );
    }
}

#[test]
fn operator_cli_dispatches_the_record_admission_arms() {
    let src = read_rel("src/operator_cli/ooda.rs");
    assert!(
        src.contains("\"record-admission\""),
        "operator_cli must route the `ooda record-admission` command"
    );
    assert!(
        src.contains("\"record-resource-admission\""),
        "operator_cli must route the `ooda record-resource-admission` command"
    );
}

#[test]
fn engineer_admission_recipe_calls_the_record_admission_tool() {
    let yaml = read_rel("prompt_assets/simard/recipes/ooda-engineer-admission.yaml");
    assert!(
        yaml.contains("ooda record-admission"),
        "ooda-engineer-admission.yaml must record its verdict by calling `simard ooda \
         record-admission`, exactly like ooda-per-goal-cycle.yaml calls `record-decision`"
    );
    for flag in ["--record-path", "--goal-id", "--cycle-number"] {
        assert!(
            yaml.contains(flag),
            "ooda-engineer-admission.yaml's tool call must pass `{flag}` (binds the record to the live ctx)"
        );
    }
}

#[test]
fn resource_admission_recipe_calls_the_record_resource_admission_tool() {
    let yaml = read_rel("prompt_assets/simard/recipes/ooda-resource-admission.yaml");
    assert!(
        yaml.contains("ooda record-resource-admission"),
        "ooda-resource-admission.yaml must record its verdict by calling `simard ooda \
         record-resource-admission`"
    );
    for flag in ["--record-path", "--goal-id", "--cycle-number"] {
        assert!(
            yaml.contains(flag),
            "ooda-resource-admission.yaml's tool call must pass `{flag}`"
        );
    }
}

#[test]
fn admission_recipes_declare_no_stdout_scraping() {
    for rel in [
        "prompt_assets/simard/recipes/ooda-engineer-admission.yaml",
        "prompt_assets/simard/recipes/ooda-resource-admission.yaml",
    ] {
        let lower = read_rel(rel).to_lowercase();
        assert!(
            lower.contains("none scraped from stdout") || lower.contains("no json"),
            "{rel} must state plainly that NOTHING is scraped from stdout — its tool call IS the effect"
        );
    }
}

#[test]
fn admission_recipes_have_no_json_decision_envelope() {
    for rel in [
        "prompt_assets/simard/recipes/ooda-engineer-admission.yaml",
        "prompt_assets/simard/recipes/ooda-resource-admission.yaml",
    ] {
        let yaml = read_rel(rel);
        assert!(
            !yaml.contains("\"decision\""),
            "{rel} must not instruct the agent to emit a `{{\"decision\": ...}}` envelope for \
             Rust to scrape (the forbidden emit→parse→act pattern)"
        );
        assert!(
            !yaml.contains("parse_admission_decision")
                && !yaml.contains("parse_resource_admission_decision"),
            "{rel} must not reference the deleted scrape helper in its header/prose"
        );
    }
}

// ===========================================================================
// GROUP E — engineer-LIFECYCLE rework (issue #4967, epic #4719).
//
// The engineer-lifecycle seam was the LAST OODA reasoner still scraping recipe
// stdout prose for its ACT effect. Group E retires it onto the same typed-record
// contract as Group A: `decide_engineer_lifecycle` runs the recipe (whose agent
// calls `simard ooda record-lifecycle-decision`) then reads a typed
// `EngineerLifecycleRecord` fail-CLOSED. These pin the *shape* of the
// post-rework sources and recipe.
//
// CRITICAL scope guard: Group E deletes ONLY the lifecycle-EXCLUSIVE scrape
// symbols. The SHARED ladder backbone (asserted above) and
// `src/recipe_output/extract.rs` (still used by out-of-scope consumers) MUST
// survive.
// ===========================================================================

#[test]
fn engineer_lifecycle_record_module_exists_and_is_reexported() {
    let path = repo_root().join("src/ooda_brain/engineer_lifecycle_record.rs");
    assert!(
        path.is_file(),
        "Group E must add the dedicated record module {} (record DTO, schema \
         const, sanitizing chokepoint, and the fail-CLOSED reader)",
        path.display()
    );
    let src = read_rel("src/ooda_brain/mod.rs");
    assert!(
        src.contains("engineer_lifecycle_record"),
        "mod.rs must declare `mod engineer_lifecycle_record;`"
    );
    for symbol in [
        "ENGINEER_LIFECYCLE_SCHEMA",
        "EngineerLifecycleRecord",
        "read_verified_engineer_lifecycle_decision",
    ] {
        assert!(
            src.contains(symbol),
            "mod.rs must re-export `{symbol}` from the engineer-lifecycle record module"
        );
    }
}

#[test]
fn recipe_brain_has_no_lifecycle_scrape_machinery() {
    let src = read_rel("src/ooda_brain/recipe_brain.rs");
    // The lifecycle-EXCLUSIVE scrape symbols Group E deletes once the seam reads
    // a typed record.
    for forbidden in [
        "parse_lifecycle_outcome",
        "parse_lifecycle_from_text",
        "extract_decision_envelope",
        "DecisionEnvelope",
        "envelope_rationale",
        "default_continue_skipping",
    ] {
        assert!(
            !src.contains(forbidden),
            "recipe_brain.rs must not contain lifecycle scrape symbol `{forbidden}` after Group E \
             — the lifecycle seam reads a typed record, it scrapes NOTHING from stdout"
        );
    }
}

#[test]
fn recipe_brain_lifecycle_seam_reads_the_typed_record() {
    let src = read_rel("src/ooda_brain/recipe_brain.rs");
    assert!(
        src.contains("read_verified_engineer_lifecycle_decision"),
        "recipe_brain.rs must run the lifecycle recipe then read the typed record via \
         `read_verified_engineer_lifecycle_decision` (modelled on Group A `read_verified_*`)"
    );
}

#[test]
fn operator_cli_dispatches_the_record_lifecycle_decision_arm() {
    let src = read_rel("src/operator_cli/ooda.rs");
    assert!(
        src.contains("\"record-lifecycle-decision\"")
            && src.contains("dispatch_record_lifecycle_decision"),
        "operator_cli must route the `ooda record-lifecycle-decision` command"
    );
}

#[test]
fn lifecycle_recipe_calls_the_record_lifecycle_decision_tool() {
    let yaml = read_rel("prompt_assets/simard/recipes/ooda-engineer-lifecycle.yaml");
    assert!(
        yaml.contains("ooda record-lifecycle-decision"),
        "ooda-engineer-lifecycle.yaml must record its decision by calling `simard ooda \
         record-lifecycle-decision` once, exactly like ooda-decide.yaml calls `record-decide`"
    );
    for flag in ["--record-path", "--goal-id", "--cycle-number", "--decision"] {
        assert!(
            yaml.contains(flag),
            "ooda-engineer-lifecycle.yaml's tool call must pass `{flag}` (binds the record to the live ctx)"
        );
    }
}

#[test]
fn lifecycle_recipe_has_no_json_envelope_or_first_word_scrape() {
    let yaml = read_rel("prompt_assets/simard/recipes/ooda-engineer-lifecycle.yaml");
    let lower = yaml.to_lowercase();
    assert!(
        !yaml.contains("\"decision\""),
        "ooda-engineer-lifecycle.yaml must not instruct the agent to emit a `{{\"decision\": ...}}` \
         envelope for Rust to scrape (the forbidden emit→parse→act pattern)"
    );
    assert!(
        !lower.contains("first word") && !lower.contains("first token"),
        "ooda-engineer-lifecycle.yaml must not instruct a first-word/first-token stdout scrape"
    );
}
