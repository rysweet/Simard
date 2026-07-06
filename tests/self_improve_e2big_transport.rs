//! Per-path E2BIG contract for the **self-improve recipe** launch (issue #2640).
//!
//! `src/bin/simard_self_improve_recipe.rs` inlines the free-text `--proposal` as
//! a single `-c proposal=<…>` argv token for `amplihack recipe run
//! simard-self-improve-cycle.yaml`. Today it truncates the proposal to 8000 chars
//! (`sanitize_context_var(…, 8000)`) to stay under ARG_MAX — E2BIG-safe but lossy:
//! a large proposal reaches the cycle maimed, and if that guard were ever removed
//! the launch would fail with E2BIG.
//!
//! The fix routes the proposal through the single spawn facade's
//! `recipe_context`, which files an oversized value and passes only
//! `-c proposal_path=<abs>` on argv — E2BIG-safe AND lossless. This test pins:
//!   1. THE BUG — an oversized proposal inlined as one argv token fails E2BIG; and
//!   2. THE FIX — `recipe_context` files it losslessly with a tiny `_path` token;
//!   3. a small proposal still inlines unchanged.
//!
//! TDD status: RED until `simard::spawn_payload` exists. Isolated integration
//! crate — the red compile does not affect the rest of the suite.

#![cfg(unix)]

use std::process::{Command, Stdio};

use simard::spawn_payload::{self, RecipeArg};

const OVERSIZED_BYTES: usize = 512 * 1024; // 0.5 MiB > 128 KiB per-arg limit
const ARGV_SAFE_CEILING: usize = 16 * 1024; // 16 KiB, far under ARG_MAX

fn oversized_proposal(marker: &str) -> String {
    format!("{marker} improvement proposal — ")
        .chars()
        .cycle()
        .take(OVERSIZED_BYTES)
        .collect()
}

/// THE BUG: inlining an oversized proposal as one `-c proposal=<…>` argv token
/// fails `exec` with E2BIG (`errno 7`) before the recipe runner starts.
#[test]
fn oversized_proposal_inlined_fails_with_e2big() {
    let payload = oversized_proposal("SELF-IMPROVE-BUG");
    assert!(
        payload.len() > 256 * 1024,
        "payload must exceed the >256KB verification threshold"
    );
    let inline_arg = format!("proposal={payload}");
    let err = Command::new("/bin/echo")
        .arg("-c")
        .arg(&inline_arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .expect_err("an oversized `-c proposal=` token must fail to exec (E2BIG)");
    assert_eq!(
        err.raw_os_error(),
        Some(7),
        "the argv-inlined oversized proposal must fail with E2BIG: {err:?}"
    );
}

/// THE FIX: an oversized proposal routed through the facade is written to a file;
/// only a short `proposal_path=<abs>` rides on argv, and the FULL proposal is
/// recoverable from the file (lossless).
#[test]
fn oversized_proposal_files_losslessly_via_facade() {
    let marker = "SELF-IMPROVE-FIX-2640";
    let payload = oversized_proposal(marker);

    let arg = spawn_payload::recipe_context("self-improve", "proposal", &payload)
        .expect("oversized proposal must file, never fail");

    let cf = match &arg {
        RecipeArg::Filed(cf) => cf,
        RecipeArg::Inline(_) => panic!("oversized proposal MUST be filed: {arg:?}"),
    };

    let value = arg.arg_value();
    assert!(
        value.starts_with("proposal_path="),
        "filed value must be `proposal_path=<abs>`: {value:?}"
    );
    assert!(
        value.len() < ARGV_SAFE_CEILING && !value.contains(marker),
        "the `-c` value must be a short path, not the inlined proposal: {} bytes",
        value.len()
    );

    let on_disk = std::fs::read_to_string(cf.path()).expect("read proposal file");
    assert_eq!(
        on_disk.len(),
        payload.len(),
        "the self-improve cycle must receive the FULL proposal — zero truncation"
    );
    assert_eq!(
        on_disk, payload,
        "round-tripped proposal must match exactly"
    );
}

/// A small proposal still inlines unchanged.
#[test]
fn small_proposal_stays_inline() {
    let arg = spawn_payload::recipe_context("self-improve", "proposal", "tune the reranker")
        .expect("small inline resolution must not fail");
    match &arg {
        RecipeArg::Inline(v) => assert_eq!(v, "proposal=tune the reranker"),
        RecipeArg::Filed(_) => panic!("a small proposal must not be filed: {arg:?}"),
    }
}
