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

// ===========================================================================
// WS-A (issue #4970) — the boolean `"{recipe}: ok"` collapse is REPLACED by a
// typed `ThreadReasoningRecord` whose natural-language `reasoning_summary` the
// rail surfaces. These source-/recipe-shape checks are authored tests-first;
// they fail RED against the pre-WS-A tree and turn GREEN only once the typed
// reasoning-record handoff lands. (Behavioural coverage lives in
// `tests_thread_reasoning_record` and `operator_cli::tests_record_thread_reasoning`.)
// ===========================================================================

/// The full 13-thread roster — every one must be recipe-backed and route its
/// outcome through `run_reflective_thread` after WS-A/B/C.
const ALL_RECIPE_BACKED_THREADS: &[&str] = &[
    "salience",
    "metacognition",
    "reflection",
    "prospection",
    "operator_model",
    "analogy",
    "narrative",
    "values_deliberation",
    "consolidation",
    "creative_ideas",
    "engineer_log_analysis",
    "interoception",
    "maintenance",
];

/// The four NEW recipe YAMLs converting the last pure-Rust threads (WS-B/WS-C).
const NEW_THREAD_RECIPES: &[&str] = &[
    "interoception-sense",
    "maintenance-housekeep",
    "engineer-log-triage",
    "creative-ideate",
];

#[test]
fn recipe_rail_no_longer_collapses_to_the_boolean_ok_string() {
    // Definition-of-Done grep gate: the `"{recipe}: ok"` collapse is deleted.
    let src = read_rel("src/cognitive_threads/recipe_rail.rs");
    for forbidden in ["{recipe_name}: ok", "{recipe}: ok", "{RECIPE}: ok"] {
        assert!(
            !src.contains(forbidden),
            "recipe_rail.rs must not contain the boolean `{forbidden}` collapse — the \
             rail surfaces the record's reasoning_summary instead"
        );
    }
}

#[test]
fn recipe_rail_reads_the_typed_reasoning_record_fail_closed() {
    let src = read_rel("src/cognitive_threads/recipe_rail.rs");
    assert!(
        src.contains("fn run_reflective_thread"),
        "recipe_rail.rs must define `run_reflective_thread` (compute path → pre-truncate → \
         invoke → read record fail-closed → surface reasoning_summary)"
    );
    assert!(
        src.contains("read_verified_thread_reasoning"),
        "run_reflective_thread must read the typed record via `read_verified_thread_reasoning` \
         (never scrape stdout)"
    );
    assert!(
        src.contains("record_path"),
        "the rail must pass `-c record_path=<abs>` so the recipe's ACT step writes there"
    );
}

#[test]
fn recipe_rail_emits_the_canonical_failure_log_format() {
    // The normative failure line pinned by the reference doc:
    //   `cognitive-thread: <thread>: FAILED — R{n} <reason>`
    let src = read_rel("src/cognitive_threads/recipe_rail.rs");
    assert!(
        src.contains("FAILED — R"),
        "recipe_rail.rs must log the canonical `FAILED — R{{n}} <reason>` format on a \
         fail-closed record read"
    );
}

#[test]
fn thread_reasoning_record_module_pins_the_contract() {
    // The shared type/reader module must exist with its pinned constants + API.
    let src = read_rel("src/ooda_brain/thread_reasoning_record.rs");
    for needle in [
        "THREAD_REASONING_SCHEMA",
        "\"thread-reasoning/v1\"",
        "MAX_AGE_SECS",
        "300",
        "enum ThreadName",
        "enum ThreadDomain",
        "fn sanitize_reasoning_summary",
        "fn read_verified_thread_reasoning",
    ] {
        assert!(
            src.contains(needle),
            "thread_reasoning_record.rs must pin `{needle}`"
        );
    }
}

#[test]
fn ooda_brain_reexports_the_thread_reasoning_surface() {
    let src = read_rel("src/ooda_brain/mod.rs");
    assert!(
        src.contains("thread_reasoning_record"),
        "ooda_brain/mod.rs must declare the `thread_reasoning_record` module"
    );
    for reexport in [
        "ThreadReasoningRecord",
        "ThreadName",
        "ThreadDomain",
        "read_verified_thread_reasoning",
        "sanitize_reasoning_summary",
    ] {
        assert!(
            src.contains(reexport),
            "ooda_brain must re-export `{reexport}` for the rail + CLI to share"
        );
    }
}

#[test]
fn cognition_cli_dispatches_record_thread_reasoning() {
    let src = read_rel("src/operator_cli/cognition.rs");
    assert!(
        src.contains("record-thread-reasoning"),
        "cognition dispatch must route the `record-thread-reasoning` subcommand"
    );
    assert!(
        src.contains("dispatch_record_thread_reasoning"),
        "cognition.rs must define `dispatch_record_thread_reasoning` (the gated writer verb)"
    );
}

#[test]
fn all_thirteen_threads_route_through_run_reflective_thread() {
    for thread in ALL_RECIPE_BACKED_THREADS {
        let src = read_rel(&format!("src/cognitive_threads/threads/{thread}.rs"));
        // A thread emits its natural-language reasoning_summary either by calling
        // `run_reflective_thread` directly (the reflective rails) or via the
        // shared `narrate_pure_thread` helper (the recipe-free rails), which
        // itself routes through `run_reflective_thread`.
        assert!(
            src.contains("run_reflective_thread") || src.contains("narrate_pure_thread"),
            "threads/{thread}.rs must route its tick through `run_reflective_thread` \
             (directly or via `narrate_pure_thread`) so it emits a natural-language \
             reasoning_summary from a typed record"
        );
    }
    // The shared helper the recipe-free rails delegate to must itself route
    // through `run_reflective_thread`, so the chain is verified end-to-end.
    let rail = read_rel("src/cognitive_threads/recipe_rail.rs");
    assert!(
        rail.contains("fn narrate_pure_thread") && rail.contains("run_reflective_thread"),
        "recipe_rail.rs `narrate_pure_thread` must route through `run_reflective_thread`"
    );
}

#[test]
fn interoception_is_no_longer_recipe_free() {
    // Tests-first, deliberate: interoception is converted to a recipe-backed
    // thread (its deterministic sensing stays in the recipe/tooling).
    let src = read_rel("src/cognitive_threads/threads/interoception.rs");
    assert!(
        src.contains("run_reflective_thread") || src.contains("narrate_pure_thread"),
        "interoception must be recipe-backed after WS-B (no longer a pure-Rust, recipe-free rail)"
    );
}

#[test]
fn every_existing_recipe_writes_the_reasoning_record() {
    // The nine already-agentic recipes gain a final ACT step calling the tool.
    for recipe in REFLECTIVE_RECIPES {
        let yaml = read_rel(&format!("prompt_assets/simard/recipes/{recipe}.yaml"));
        assert!(
            yaml.contains("record-thread-reasoning"),
            "{recipe}.yaml must call `simard cognition record-thread-reasoning` as its ACT step \
             so its reasoning surfaces via a typed record"
        );
    }
}

#[test]
fn the_four_new_thread_recipes_exist_and_record_reasoning() {
    for recipe in NEW_THREAD_RECIPES {
        let yaml = read_rel(&format!("prompt_assets/simard/recipes/{recipe}.yaml"));
        assert!(
            yaml.contains("simard cognition record-thread-reasoning"),
            "{recipe}.yaml must record its reasoning via the shared `record-thread-reasoning` tool"
        );
    }
}
