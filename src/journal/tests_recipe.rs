//! Tests for the external-service (recipe-runner) integration seams (issue #2606).
//!
//! The end-to-end recipe path needs a live agent/`recipe-runner-rs`, so it is
//! not exercised here. What *is* deterministically testable — and what actually
//! breaks if the external contract drifts — are the offline seams:
//!
//! * [`day_context_json`]: the JSON payload Simard hands the draft recipe.
//! * [`extract_recipe_output`]: parsing/validating the runner's JSON envelope.
//! * [`resolve_recipe_path`]: locating the recipe asset on disk.
//!
//! These lock the data contract so a silent drift fails loudly in CI rather than
//! only at a live journal tick.

use std::fs;

use serde_json::Value;

use crate::journal::providers::episode_time_label;
use crate::journal::recipe::{day_context_json, extract_recipe_output, resolve_recipe_path};
use crate::journal::test_support::{day, episode_at, pr};
use crate::journal::types::{DayContext, MemoryGrowth};

/// Parse the payload back into a JSON value so tests assert on structure, not on
/// brittle string formatting.
fn payload(day: &DayContext) -> Value {
    serde_json::from_str(&day_context_json(day)).expect("day_context_json must emit valid JSON")
}

#[test]
fn day_context_json_is_valid_json_with_the_expected_shape() {
    let mut ctx = DayContext::new(day());
    ctx.episodes = vec![episode_at("looked into the flaky check", 5)];
    ctx.goals = vec!["ship the journal".to_string()];
    ctx.deploys = vec!["rolled out the dashboard".to_string()];
    ctx.facts = vec!["the cache was cold on boot".to_string()];
    ctx.triggers = vec!["review the overnight run".to_string()];
    ctx.procedures = vec!["how to rotate the token".to_string()];
    ctx.notable = vec!["a long-running job finished".to_string()];
    ctx.overseer_events = vec!["the steward pruned stale goals".to_string()];

    let v = payload(&ctx);
    assert_eq!(v["date"], "2026-07-05");
    // Prepared-context substance is passed verbatim (arrays of strings), not as
    // bare counts — the model summarises *what* they were.
    assert_eq!(v["facts"][0], "the cache was cold on boot");
    assert_eq!(v["triggers"][0], "review the overnight run");
    assert_eq!(v["procedures"][0], "how to rotate the token");
    assert_eq!(v["notable"][0], "a long-running job finished");
    assert_eq!(v["goals"][0], "ship the journal");
    assert_eq!(v["deploys"][0], "rolled out the dashboard");
    assert_eq!(v["overseer_events"][0], "the steward pruned stale goals");
}

#[test]
fn day_context_json_sorts_episodes_oldest_first_with_time_labels() {
    let mut ctx = DayContext::new(day());
    // Intentionally out of order — the payload must sort them chronologically.
    ctx.episodes = vec![
        episode_at("newer moment", 30),
        episode_at("older moment", 10),
    ];

    let v = payload(&ctx);
    let eps = v["episodes"].as_array().expect("episodes array");
    assert_eq!(eps.len(), 2);
    assert_eq!(eps[0]["content"], "older moment");
    assert_eq!(eps[1]["content"], "newer moment");
    // Each moment carries the same human-readable label the report renders.
    assert_eq!(eps[0]["time"], episode_time_label(10));
    assert_eq!(eps[1]["time"], episode_time_label(30));
}

#[test]
fn day_context_json_serialises_prs_and_memory_growth() {
    let mut ctx = DayContext::new(day());
    ctx.prs = vec![pr(42, "made the checks faster", "merged")];
    ctx.memory_growth = Some(MemoryGrowth {
        facts_added: 3,
        episodes_added: 7,
    });

    let v = payload(&ctx);
    assert_eq!(v["prs"][0]["number"], 42);
    assert_eq!(v["prs"][0]["summary"], "made the checks faster");
    assert_eq!(v["prs"][0]["outcome"], "merged");
    assert_eq!(v["memory_growth"]["facts_added"], 3);
    assert_eq!(v["memory_growth"]["episodes_added"], 7);
}

#[test]
fn day_context_json_for_a_quiet_day_has_empty_collections_and_null_growth() {
    let v = payload(&DayContext::new(day()));
    assert_eq!(v["date"], "2026-07-05");
    assert!(v["episodes"].as_array().expect("episodes array").is_empty());
    assert!(v["prs"].as_array().expect("prs array").is_empty());
    assert!(v["facts"].as_array().expect("facts array").is_empty());
    assert!(v["memory_growth"].is_null());
}

#[test]
fn extract_recipe_output_returns_the_final_steps_trimmed_text() {
    let stdout = br#"{
        "success": true,
        "step_results": [
            {"output": "draft"},
            {"output": "  the final report  \n"}
        ]
    }"#;
    let out = extract_recipe_output(stdout).expect("valid envelope yields the last output");
    assert_eq!(out, "the final report");
}

#[test]
fn extract_recipe_output_rejects_a_success_false_envelope() {
    let stdout = br#"{"success": false, "step_results": [{"output": "anything"}]}"#;
    let err = extract_recipe_output(stdout).expect_err("success=false must be an error");
    assert!(
        err.to_string().contains("success=false"),
        "unexpected error: {err}"
    );
}

#[test]
fn extract_recipe_output_rejects_empty_final_output() {
    let stdout = br#"{"success": true, "step_results": [{"output": "   \n  "}]}"#;
    let err = extract_recipe_output(stdout).expect_err("blank output must be an error");
    assert!(
        err.to_string().contains("no output"),
        "unexpected error: {err}"
    );
}

#[test]
fn extract_recipe_output_rejects_an_envelope_with_no_steps() {
    let stdout = br#"{"success": true, "step_results": []}"#;
    let err = extract_recipe_output(stdout).expect_err("no steps must be an error");
    assert!(
        err.to_string().contains("no output"),
        "unexpected error: {err}"
    );
}

#[test]
fn extract_recipe_output_rejects_malformed_json() {
    let err = extract_recipe_output(b"not json at all").expect_err("bad JSON must be an error");
    assert!(
        err.to_string().contains("JSON envelope"),
        "unexpected error: {err}"
    );
}

#[test]
fn resolve_recipe_path_finds_an_in_tree_recipe_and_none_otherwise() {
    let root = tempfile::tempdir().expect("tempdir");
    let recipes = root.path().join("prompt_assets/simard/recipes");
    fs::create_dir_all(&recipes).expect("create recipe dir");
    // Unique name so the `~/.simard` hot-reload branch can never shadow it.
    let name = "journal-test-fixture-1a2b3c.yaml";
    let file = recipes.join(name);
    fs::write(&file, "name: fixture\n").expect("write recipe");

    assert_eq!(
        resolve_recipe_path(root.path(), name),
        Some(file),
        "an in-tree recipe must resolve"
    );
    assert_eq!(
        resolve_recipe_path(root.path(), "no-such-recipe-xyz.yaml"),
        None,
        "a missing recipe must resolve to None (the deterministic fallback)"
    );
}
