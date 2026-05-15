//! Integration tests for the `simard-engineer-loop-recipe` helper bin.
//!
//! This bin is a thin shell-out: it parses --workspace / --objective /
//! --topology / --state-root, then `Command::new("amplihack").status()`.
//! We exercise every flag-parsing branch (lines 13–46 in
//! `src/bin/simard_engineer_loop_recipe.rs`) plus the spawn-failure branch
//! (lines 73–77) by pointing PATH at an empty directory so the `amplihack`
//! exec lookup fails.
//!
//! Filed against rysweet/Simard#1749.

use assert_cmd::Command;
use tempfile::TempDir;

fn bin() -> Command {
    Command::cargo_bin("simard-engineer-loop-recipe")
        .expect("simard-engineer-loop-recipe must build")
}

#[test]
fn no_args_prints_usage_and_exits_2() {
    let assert = bin().assert().code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("usage: simard-engineer-loop-recipe"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("--workspace"), "stderr: {stderr}");
    assert!(stderr.contains("--objective"), "stderr: {stderr}");
    assert!(stderr.contains("--topology"), "stderr: {stderr}");
    assert!(stderr.contains("--state-root"), "stderr: {stderr}");
}

#[test]
fn missing_objective_exits_2() {
    let assert = bin().args(["--workspace", "/tmp"]).assert().code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(stderr.contains("missing --objective"), "stderr: {stderr}");
}

#[test]
fn missing_state_root_exits_2() {
    let assert = bin()
        .args(["--workspace", "/tmp", "--objective", "do-thing"])
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(stderr.contains("missing --state-root"), "stderr: {stderr}");
}

#[test]
fn topology_defaults_when_unspecified_and_amplihack_spawn_fails_cleanly() {
    // Point PATH at an empty directory so `amplihack` can't be exec'd.
    // The bin should reach the spawn step and report a clean error
    // (either "failed to spawn amplihack" if exec returns Err, or
    // "amplihack recipe run failed" if it returns a non-zero status).
    let empty_path = TempDir::new().unwrap();

    let output = bin()
        .env("PATH", empty_path.path())
        .env_remove("SIMARD_ENGINEER_RECIPE_PATH")
        .args([
            "--workspace",
            "/tmp",
            "--objective",
            "test",
            "--state-root",
            "/tmp",
        ])
        .output()
        .expect("bin must run");

    assert!(!output.status.success(), "must fail when amplihack missing");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to spawn amplihack")
            || stderr.contains("amplihack recipe run failed"),
        "expected spawn-failure or run-failure message, got: {stderr}"
    );
}

#[test]
fn explicit_topology_is_accepted_and_amplihack_spawn_fails_cleanly() {
    let empty_path = TempDir::new().unwrap();

    let output = bin()
        .env("PATH", empty_path.path())
        .env_remove("SIMARD_ENGINEER_RECIPE_PATH")
        .args([
            "--workspace",
            "/tmp",
            "--objective",
            "test",
            "--topology",
            "multi-process",
            "--state-root",
            "/tmp",
        ])
        .output()
        .expect("bin must run");

    assert!(!output.status.success(), "must fail when amplihack missing");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to spawn amplihack")
            || stderr.contains("amplihack recipe run failed"),
        "stderr: {stderr}"
    );
}

#[test]
fn custom_recipe_path_env_is_consumed() {
    // Sets SIMARD_ENGINEER_RECIPE_PATH to confirm the env-read branch is
    // exercised. Spawn still fails (no amplihack on PATH) → clean error.
    let empty_path = TempDir::new().unwrap();

    let output = bin()
        .env("PATH", empty_path.path())
        .env("SIMARD_ENGINEER_RECIPE_PATH", "/some/custom/recipe.yaml")
        .args([
            "--workspace",
            "/tmp",
            "--objective",
            "test",
            "--topology",
            "single-process",
            "--state-root",
            "/tmp",
        ])
        .output()
        .expect("bin must run");

    assert!(!output.status.success(), "must fail when amplihack missing");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to spawn amplihack")
            || stderr.contains("amplihack recipe run failed"),
        "stderr: {stderr}"
    );
}
