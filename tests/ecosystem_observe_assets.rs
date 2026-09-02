//! Content-pin + loadability tests for the live agentic `ecosystem-observe`
//! chain (issue #2419).
//!
//! Simard observes her stewarded ecosystem with a DETERMINISTIC WORKFLOW OF
//! AGENTIC STEPS + PROMPTS — not a Rust "code sensor." These tests pin the
//! contract the thin rail (`src/overseer/ecosystem_observe.rs`) depends on
//! WITHOUT running the agent: the recipe exists and is valid runner YAML, its
//! two agent steps carry the OBSERVE→BRIEF semantic handoff through the
//! `{{observed_problems_path}}` context-file var (NOT a `{{step_output}}`
//! interpolation), the roster is the single source of truth, and the prompts no
//! longer carry the "#2419 not wired live" banner.
//!
//! Mirrors `tests/creative_ideas_dedup_assets.rs`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn asset(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("asset {} must be readable: {e}", path.display()))
}

const RECIPE: &str = "prompt_assets/simard/recipes/ecosystem-observe.yaml";
const OBSERVE_PROMPT: &str = "prompt_assets/simard/overseer/observe.md";
const BRIEF_PROMPT: &str = "prompt_assets/simard/overseer/problem_to_brief.md";
const ROSTER_SEED: &str = "prompt_assets/simard/identity/stewarded_repos.seed.toml";

/// The recipe exposes exactly the context vars the thin rail renders (all `_path`
/// values ride `ContextFile`), plus the rail-owned `escalation_note`.
#[test]
fn recipe_exposes_rail_context_vars() {
    let body = asset(RECIPE);
    for var in [
        "{{roster_path}}",
        "{{inflight_refs_path}}",
        "{{observed_problems_path}}",
        "{{escalation_note}}",
    ] {
        assert!(
            body.contains(var),
            "ecosystem-observe recipe must reference the rail context var {var}"
        );
    }
}

/// Two agent steps named OBSERVE and BRIEF, each a `default` reasoning agent,
/// each naming its terminal output.
#[test]
fn recipe_is_two_default_agent_steps() {
    let body = asset(RECIPE);
    for id in ["id: \"observe\"", "id: \"brief\""] {
        assert!(body.contains(id), "recipe must define step {id}");
    }
    assert_eq!(
        body.matches("agent: \"default\"").count(),
        2,
        "both OBSERVE and BRIEF must be default-agent reasoning steps"
    );
    for out in ["output: \"observe_result\"", "output: \"ecosystem_briefs\""] {
        assert!(body.contains(out), "recipe must name terminal output {out}");
    }
}

/// The OBSERVE→BRIEF handoff rides the `{{observed_problems_path}}` context file
/// (the proven `ContextFile` transport), NOT a `{{step_output}}` interpolation.
/// OBSERVE writes it; BRIEF reads it; no Rust ever parses it.
#[test]
fn handoff_is_a_context_file_path_not_a_step_output() {
    let body = asset(RECIPE);
    // The shared handoff path appears in BOTH steps (OBSERVE writes, BRIEF reads).
    assert!(
        body.matches("{{observed_problems_path}}").count() >= 2,
        "the observed_problems_path handoff must be referenced by both steps"
    );
    // No step interpolates a prior step's captured `output:` as {{observe_result}}.
    assert!(
        !body.contains("{{observe_result}}"),
        "the handoff must NOT ride a step-output interpolation — use the context file"
    );
}

/// The recipe is valid runner YAML. When `recipe-runner-rs` is on PATH we assert
/// `--validate-only` succeeds; otherwise the test degrades to a structural check
/// (so it never flakes in an environment without the binary).
#[test]
fn recipe_is_valid_runner_yaml() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(RECIPE);
    match Command::new("recipe-runner-rs")
        .arg(&path)
        .arg("--validate-only")
        .output()
    {
        Ok(out) => assert!(
            out.status.success(),
            "recipe-runner-rs --validate-only must accept ecosystem-observe.yaml:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ),
        Err(_) => {
            // Binary unavailable: fall back to a structural sanity check.
            let body = asset(RECIPE);
            assert!(body.contains("name: \"ecosystem-observe\""));
            assert!(body.contains("steps:"));
        }
    }
}

/// The `#2419 not wired live` banner is REMOVED from both prompts — they are now
/// loaded and run by the live recipe.
#[test]
fn prompts_have_no_not_wired_live_banner() {
    for prompt in [OBSERVE_PROMPT, BRIEF_PROMPT] {
        let body = asset(prompt);
        assert!(
            !body.contains("not wired live"),
            "{prompt} must not carry the '#2419 not wired live' banner once wired"
        );
        assert!(
            !body.contains("design scaffolding"),
            "{prompt} must not carry the 'design scaffolding' banner once wired"
        );
    }
}

/// The OBSERVE prompt is generalized to the multi-repo ecosystem: it scans a
/// roster with `gh`, keeps Simard's own process-health signals, dedups against
/// in-flight work, and writes to the shared handoff path.
#[test]
fn observe_prompt_is_multi_repo_and_agentic() {
    let body = asset(OBSERVE_PROMPT);
    for token in [
        "{{roster_path}}",
        "{{inflight_refs_path}}",
        "{{observed_problems_path}}",
        "simard status",
        "gh",
        "rysweet",
        "XPIA",
    ] {
        assert!(
            body.contains(token),
            "generalized observe prompt must reference {token}"
        );
    }
}

/// The stewarded-roster SEED is identity DATA and the default source of truth
/// for a fresh identity: it lists the 10 stewarded slugs (as generic curated
/// `[[item]]` keys) and deliberately excludes the deprecated Python
/// `rysweet/amplihack`. The durable, mutable roster is seeded from this file on
/// first use and then owned as identity-scoped state.
#[test]
fn roster_seed_is_the_default_source_of_truth() {
    let body = asset(ROSTER_SEED);
    for slug in [
        "rysweet/Simard",
        "rysweet/RustyClawd",
        "rysweet/amplihack-rs",
        "rysweet/azlin",
        "rysweet/amplihack-memory-lib",
        "rysweet/amplihack-agent-eval",
        "rysweet/agent-kgpacks",
        "rysweet/amplihack-recipe-runner",
        "rysweet/amplihack-xpia-defender",
        "rysweet/gadugi-agentic-test",
    ] {
        assert!(
            body.contains(slug),
            "roster seed must list stewarded repo {slug}"
        );
    }
    assert!(
        !body.contains("key = \"rysweet/amplihack\""),
        "roster seed must NOT list the deprecated Python rysweet/amplihack"
    );
}
