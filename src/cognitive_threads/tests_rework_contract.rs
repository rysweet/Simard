//! Source- and recipe-level guardrails for the PR #3142 rework (issue #3142).
//!
//! These are **executable acceptance checks authored tests-first** for the
//! operator directive: a reflective thread's recipe must reason AND act through
//! its own `simard …` tools, exactly like `distill-episodes.yaml`. The forbidden
//! pattern — "recipe emits JSON → Rust scrapes stdout → Rust performs the write"
//! — must be gone. Unlike the behavioural tests in [`super::tests_catalog`],
//! these read the on-disk sources/recipes directly, so they also pin the *shape*
//! of the code and prompts (the exact greps in the rework brief's acceptance
//! list), not just runtime behaviour.
//!
//! They fail RED against the pre-rework tree (the recipes still print a strict
//! JSON envelope and `recipe_rail` still scrapes it) and turn GREEN only once the
//! rework lands.

use std::path::PathBuf;

/// Repo root (the crate manifest dir), so a test can read a source/recipe file
/// by its repo-relative path regardless of the process CWD.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Read a repo-relative file to a string (panics with the path on failure so a
/// missing file is a loud, obvious failure).
fn read_rel(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The nine reflective-thread recipes reworked to act via tools.
const REFLECTIVE_RECIPES: &[&str] = &[
    "reflect-postmortem",
    "metacognition-appraise",
    "salience-appraise",
    "prospect-foresight",
    "operator-model",
    "consolidate-sleep",
    "analogy-map",
    "narrative-identity",
    "values-deliberate",
];

/// The nine reflective-thread rail sources whose parse-then-act layer is deleted.
/// (`interoception` is deterministic and recipe-free, handled separately.)
const REFLECTIVE_THREADS: &[&str] = &[
    "reflection",
    "metacognition",
    "salience",
    "narrative",
    "consolidation",
    "prospection",
    "values_deliberation",
    "analogy",
    "operator_model",
];

// ---------------------------------------------------------------------------
// recipe_rail.rs — the JSON-parse-then-act layer is DELETED
// ---------------------------------------------------------------------------

#[test]
fn recipe_rail_has_no_json_parse_or_goal_proposal_layer() {
    let src = read_rel("src/cognitive_threads/recipe_rail.rs");
    for forbidden in [
        "fn classify_recipe_stdout",
        "fn parse_step_output",
        "fn invoke_for_envelope",
        "fn propose_goal_if_capacity",
        "extract_json_payload",
        "InvokeResult::Json",
        "InvokeResult::SemanticMiss",
    ] {
        assert!(
            !src.contains(forbidden),
            "recipe_rail.rs must not contain `{forbidden}` after the rework — the \
             invoker is a pure success/failure runner that parses NOTHING from stdout"
        );
    }
}

#[test]
fn recipe_rail_invoke_result_is_success_or_failure_only() {
    let src = read_rel("src/cognitive_threads/recipe_rail.rs");
    assert!(
        src.contains("enum InvokeResult"),
        "InvokeResult still exists as the invoker verdict"
    );
    assert!(
        src.contains("Ran") && src.contains("Failed"),
        "InvokeResult is reduced to Ran / Failed (success/failure) only"
    );
}

#[test]
fn recipe_rail_exports_the_memory_socket_to_the_child() {
    // The recipe subprocess must inherit SIMARD_MEMORY_SOCKET so a bare `simard
    // memory remember` reaches the live daemon — the distill-episodes seam.
    let src = read_rel("src/cognitive_threads/recipe_rail.rs");
    assert!(
        src.contains("SIMARD_MEMORY_SOCKET"),
        "the reworked invoker exports SIMARD_MEMORY_SOCKET into the recipe child"
    );
}

// ---------------------------------------------------------------------------
// threads/*.rs — no envelope parse, no direct durable writes
// ---------------------------------------------------------------------------

#[test]
fn reflective_threads_do_not_parse_recipe_output() {
    for thread in REFLECTIVE_THREADS {
        let src = read_rel(&format!("src/cognitive_threads/threads/{thread}.rs"));
        for forbidden in [
            "envelope.get(",
            "invoke_for_envelope",
            "extract_json_payload",
        ] {
            assert!(
                !src.contains(forbidden),
                "threads/{thread}.rs must not parse recipe output (`{forbidden}`) — it \
                 records ran/health from the recipe's EXIT STATUS only"
            );
        }
    }
}

#[test]
fn reflective_threads_perform_no_direct_durable_writes() {
    // Every durable write now happens inside the recipe's `simard …` tool calls,
    // so no reflective thread may write memory, a goal, or the signal file itself.
    for thread in REFLECTIVE_THREADS {
        let src = read_rel(&format!("src/cognitive_threads/threads/{thread}.rs"));
        for forbidden in [
            "store_fact",
            "store_procedure",
            "salience_signal::write_signal",
            "propose_goal_if_capacity",
        ] {
            assert!(
                !src.contains(forbidden),
                "threads/{thread}.rs must not call `{forbidden}` after the rework — the \
                 recipe owns every durable effect"
            );
        }
    }
}

#[test]
fn reflective_threads_have_no_unwrap_or_silent_defaults_on_recipe_fields() {
    // A recipe failure must be recorded LOUDLY, never squashed into a
    // "ran, wrote nothing" success via `.unwrap_or(..)` on a parsed field.
    for thread in REFLECTIVE_THREADS {
        let src = read_rel(&format!("src/cognitive_threads/threads/{thread}.rs"));
        assert!(
            !src.contains(".unwrap_or("),
            "threads/{thread}.rs must not use `.unwrap_or(` silent defaults after the rework"
        );
    }
}

// ---------------------------------------------------------------------------
// recipes/*.yaml — act via tools, print NO JSON envelope
// ---------------------------------------------------------------------------

#[test]
fn reflective_recipes_call_a_simard_tool() {
    for recipe in REFLECTIVE_RECIPES {
        let yaml = read_rel(&format!("prompt_assets/simard/recipes/{recipe}.yaml"));
        let calls_a_tool = yaml.contains("simard memory remember")
            || yaml.contains("simard memory remember-procedure")
            || yaml.contains("simard goal add")
            || yaml.contains("simard cognition salience-signal");
        assert!(
            calls_a_tool,
            "{recipe}.yaml must perform its effect by calling a `simard …` tool \
             (remember / remember-procedure / goal add / cognition salience-signal), \
             exactly like distill-episodes.yaml"
        );
    }
}

#[test]
fn reflective_recipes_print_no_json_envelope() {
    for recipe in REFLECTIVE_RECIPES {
        let yaml = read_rel(&format!("prompt_assets/simard/recipes/{recipe}.yaml"));
        let lower = yaml.to_lowercase();
        assert!(
            lower.contains("no json") || lower.contains("no output file"),
            "{recipe}.yaml must state plainly that there is no JSON / no output file to \
             print — its tool calls ARE the effect"
        );
        assert!(
            !yaml.contains("Return **only**"),
            "{recipe}.yaml must not instruct the agent to `Return **only**` a JSON \
             object for Rust to scrape (the forbidden emit→parse→act pattern)"
        );
    }
}

#[test]
fn salience_appraise_recipe_calls_the_salience_signal_tool() {
    let yaml = read_rel("prompt_assets/simard/recipes/salience-appraise.yaml");
    assert!(
        yaml.contains("simard cognition salience-signal"),
        "salience-appraise.yaml must write the numeric Decide signal via the clamping \
         `simard cognition salience-signal` tool, not by emitting JSON for Rust to parse"
    );
}

// ---------------------------------------------------------------------------
// operator_cli — the `cognition salience-signal` subcommand is dispatched
// ---------------------------------------------------------------------------

#[test]
fn operator_cli_dispatches_the_cognition_command() {
    let src = read_rel("src/operator_cli/mod.rs");
    assert!(
        src.contains("\"cognition\""),
        "operator_cli dispatch must route the `cognition` command (for `simard \
         cognition salience-signal`)"
    );
}
