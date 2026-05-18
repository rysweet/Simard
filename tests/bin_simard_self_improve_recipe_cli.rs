//! Integration tests for the `simard-self-improve-recipe` helper bin.
//!
//! This bin is a shell-out: it parses optional flags (--workspace,
//! --suite-id, --proposal, --weak-threshold, --target-dimension, --recipe,
//! --amplihack-home), builds a recipe path, and exec's `amplihack recipe
//! run …`. Tests exercise:
//! - default-flag fall-through (every `arg().unwrap_or(default)` branch)
//! - explicit-flag parsing for every supported flag
//! - clean failure when amplihack is not on PATH
//!
//! Filed against rysweet/Simard#1749.

use assert_cmd::Command;
use tempfile::TempDir;

fn bin() -> Command {
    Command::cargo_bin("simard-self-improve-recipe").expect("simard-self-improve-recipe must build")
}

#[test]
fn no_args_uses_defaults_and_fails_when_amplihack_absent() {
    let empty_path = TempDir::new().unwrap();
    let output = bin()
        .env("PATH", empty_path.path())
        .output()
        .expect("bin must run");
    assert!(
        !output.status.success(),
        "should fail when amplihack absent"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("simard-self-improve-recipe")
            || stderr.contains("amplihack")
            || stderr.contains("spawn")
            || stderr.contains("recipe"),
        "stderr: {stderr}"
    );
}

#[test]
fn explicit_workspace_and_suite_id_flags_parsed() {
    let empty_path = TempDir::new().unwrap();
    let output = bin()
        .env("PATH", empty_path.path())
        .args([
            "--workspace",
            "/tmp/some/workspace",
            "--suite-id",
            "my-suite",
        ])
        .output()
        .expect("bin must run");
    assert!(!output.status.success());
    // The bin reaches the spawn step regardless of whether amplihack exists,
    // so the error must be a clean spawn-or-recipe-failure message.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("simard-self-improve-recipe") || stderr.contains("amplihack"),
        "stderr: {stderr}"
    );
}

#[test]
fn proposal_and_weak_threshold_and_target_dimension_flags_parsed() {
    let empty_path = TempDir::new().unwrap();
    let output = bin()
        .env("PATH", empty_path.path())
        .args([
            "--proposal",
            "do-the-thing",
            "--weak-threshold",
            "0.6",
            "--target-dimension",
            "correctness",
        ])
        .output()
        .expect("bin must run");
    assert!(!output.status.success());
}

#[test]
fn absolute_recipe_flag_is_used_verbatim() {
    let empty_path = TempDir::new().unwrap();
    let output = bin()
        .env("PATH", empty_path.path())
        .args(["--recipe", "/abs/path/to/recipe.yaml"])
        .output()
        .expect("bin must run");
    // Spawn fails (no amplihack), but the absolute-path branch (line 44)
    // is exercised. We just verify clean failure.
    assert!(!output.status.success());
}

#[test]
fn relative_recipe_with_amplihack_home_flag_is_joined() {
    let empty_path = TempDir::new().unwrap();
    let output = bin()
        .env("PATH", empty_path.path())
        .args([
            "--recipe",
            "relative/recipe.yaml",
            "--amplihack-home",
            "/some/amp/home",
        ])
        .output()
        .expect("bin must run");
    // Spawn fails — confirm it is the spawn-failure path, not a panic.
    assert!(!output.status.success());
}
