//! Per-path E2BIG contract for the **Overseer fix-launch** (`amplihack recipe
//! run smart-orchestrator …`), issue #2640.
//!
//! `overseer::launch::smart_orchestrator_args` inlines the free-text
//! `task_description` as a single `-c task_description=<…>` argv token. Today it
//! defends against E2BIG by *truncating* the value to 8000 chars
//! (`sanitize_context_var(…, 8000)`) — which keeps `exec` alive but silently
//! DROPS context past 8 KiB (a G3 "no truncation" violation): a large brief
//! reaches the orchestrator maimed.
//!
//! The fix routes the value through the single spawn facade's
//! `recipe_context`, which files any value ≥ `ARGV_PAYLOAD_MAX_BYTES` and passes
//! only `-c task_description_path=<abs>` on argv — E2BIG-safe AND lossless. This
//! test pins:
//!   1. the current builder truncates (documents the defect the fix removes),
//!   2. `recipe_context` files an oversized `task_description` losslessly with a
//!      tiny `<key>_path` argv token, and
//!   3. a small `task_description` still inlines unchanged.
//!
//! TDD status: RED until `simard::spawn_payload` exists. Isolated integration
//! crate — the red compile does not affect the rest of the suite.

#![cfg(unix)]

use simard::overseer::capabilities::RecipeBrief;
use simard::overseer::launch::smart_orchestrator_args;
use simard::spawn_payload::{self, RecipeArg};

const OVERSIZED_BYTES: usize = 512 * 1024; // 0.5 MiB > 128 KiB per-arg limit
const ARGV_SAFE_CEILING: usize = 16 * 1024; // 16 KiB, far under ARG_MAX

fn oversized_task(marker: &str) -> String {
    format!("{marker} task_description — ")
        .chars()
        .cycle()
        .take(OVERSIZED_BYTES)
        .collect()
}

/// Documents the defect: the current builder inlines `task_description` and, to
/// stay under ARG_MAX, TRUNCATES an oversized brief — so the orchestrator never
/// sees the full context. The fix replaces this lossy inline with a lossless
/// file channel (below).
#[test]
fn current_builder_truncates_oversized_task_description() {
    let marker = "OVERSEER-TRUNC-2640";
    let brief = RecipeBrief {
        task_description: oversized_task(marker),
        target_repo: "rysweet/Simard".to_string(),
        sequence_group: None,
    };
    let args = smart_orchestrator_args(&brief);
    let td = args
        .iter()
        .find(|a| a.starts_with("task_description="))
        .expect("current builder inlines a task_description= token");
    assert!(
        td.len() < 32 * 1024,
        "precondition: the current builder truncates the oversized task_description \
         (it is far smaller than the {OVERSIZED_BYTES}-byte input) — this lossy \
         behaviour is what the file channel replaces: {} bytes",
        td.len()
    );
}

/// THE FIX: an oversized `task_description` routed through the facade is written
/// to a file; only a short `task_description_path=<abs>` rides on argv, and the
/// FULL brief is recoverable from the file (lossless — no 8000-char truncation).
#[test]
fn oversized_task_description_files_losslessly_via_facade() {
    let marker = "OVERSEER-FIX-2640";
    let payload = oversized_task(marker);
    assert!(
        payload.len() > 256 * 1024,
        "payload must exceed the >256KB verification threshold"
    );

    let arg = spawn_payload::recipe_context("overseer", "task_description", &payload)
        .expect("oversized task_description must file, never fail");

    let cf = match &arg {
        RecipeArg::Filed(cf) => cf,
        RecipeArg::Inline(_) => panic!("oversized task_description MUST be filed: {arg:?}"),
    };

    let value = arg.arg_value();
    assert!(
        value.starts_with("task_description_path="),
        "filed value must be `task_description_path=<abs>`: {value:?}"
    );
    assert!(
        value.len() < ARGV_SAFE_CEILING && !value.contains(marker),
        "the `-c` value must be a short path, not the inlined brief: {} bytes",
        value.len()
    );

    let on_disk = std::fs::read_to_string(cf.path()).expect("read task file");
    assert_eq!(
        on_disk.len(),
        payload.len(),
        "the orchestrator must receive the FULL brief — zero truncation"
    );
    assert_eq!(
        on_disk, payload,
        "round-tripped task_description must match exactly"
    );
}

/// A small `task_description` still inlines unchanged (`key=value`), so the
/// common case keeps a single argv token and needs no recipe-asset `_path` read.
#[test]
fn small_task_description_stays_inline() {
    let arg = spawn_payload::recipe_context("overseer", "task_description", "fix the banner")
        .expect("small inline resolution must not fail");
    match &arg {
        RecipeArg::Inline(v) => assert_eq!(v, "task_description=fix the banner"),
        RecipeArg::Filed(_) => panic!("a small brief must not be filed: {arg:?}"),
    }
}

/// Assembling the recipe argv with the filed token stays far under ARG_MAX even
/// with a 0.5 MiB brief backing it — ARG_MAX safety by construction.
#[test]
fn overseer_recipe_argv_is_arg_max_safe_with_a_huge_brief() {
    let payload = oversized_task("OVERSEER-ARGMAX");
    let arg =
        spawn_payload::recipe_context("overseer", "task_description", &payload).expect("file");

    let argv = [
        "recipe".to_string(),
        "run".to_string(),
        "amplifier-bundle/recipes/smart-orchestrator.yaml".to_string(),
        "-c".to_string(),
        arg.arg_value(),
        "-c".to_string(),
        "target_repo=rysweet/Simard".to_string(),
    ];
    let total: usize = argv.iter().map(|a| a.len() + 1).sum();
    assert!(
        total < ARGV_SAFE_CEILING,
        "overseer recipe argv must stay far under ARG_MAX with a 0.5 MiB brief: \
         {total} bytes >= {ARGV_SAFE_CEILING}"
    );
}
