//! Integration tests for the `simard-gastronome` kitchen app bin.
//!
//! Exercises the observable CLI surface end-to-end — help, the built-in demo,
//! text/JSON output, planning a real bundle file, strict-mode exit codes, and
//! error handling — plus a guard that the shipped example bundle
//! (`docs/examples/gastronome-harvest-dinner.json`) stays valid.

use std::path::PathBuf;

use assert_cmd::Command;
use simard::gastronome::{KitchenBrief, MenuPlan, demo_bundle};

fn bin() -> Command {
    Command::cargo_bin("simard-gastronome").expect("simard-gastronome must build")
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn no_args_prints_help_and_succeeds() {
    let assert = bin().assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        stdout.contains("USAGE"),
        "help should show usage, got: {stdout}"
    );
    assert!(stdout.contains("--demo"));
}

#[test]
fn demo_text_output_has_expected_sections() {
    let assert = bin().arg("--demo").assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(stdout.contains("Menu & cost"));
    assert!(stdout.contains("Prep schedule"));
    assert!(stdout.contains("Nutrition per guest"));
    assert!(stdout.contains("Total cost"));
}

#[test]
fn demo_json_output_parses_as_menu_plan() {
    let assert = bin()
        .args(["--demo", "--format", "json"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let plan: MenuPlan = serde_json::from_str(&stdout).expect("stdout must be a MenuPlan");
    assert_eq!(plan.guests, 12);
    assert_eq!(plan.items.len(), 3);
    assert!(plan.total_cost > 0.0);
    // roast-chicken critical path (15+75+15) dominates the schedule.
    assert_eq!(plan.schedule.total_lead_minutes, 105);
}

#[test]
fn plan_reads_a_bundle_file() {
    // Serialise the built-in demo to a temp file and plan it via `plan <path>`.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bundle.json");
    std::fs::write(&path, serde_json::to_string(&demo_bundle()).unwrap()).unwrap();

    let assert = bin().arg("plan").arg(&path).assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(stdout.contains("Harvest dinner"));
}

#[test]
fn shipped_example_bundle_plans_cleanly() {
    let example = repo_root().join("docs/examples/gastronome-harvest-dinner.json");
    let raw = std::fs::read_to_string(&example).expect("example bundle must exist");
    // It must be a valid bundle...
    let bundle: KitchenBrief = serde_json::from_str(&raw).expect("example bundle must parse");
    assert!(!bundle.recipes.is_empty());
    // ...and the CLI must plan it within budget and dietary-compliant (--strict).
    bin()
        .arg("plan")
        .arg(&example)
        .arg("--strict")
        .assert()
        .success();
}

#[test]
fn strict_flags_over_budget_with_exit_code_3() {
    // Take the demo but set an impossibly tight budget so the plan is over.
    let mut bundle = demo_bundle();
    bundle.brief.budget_total = Some(1.0);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tight.json");
    std::fs::write(&path, serde_json::to_string(&bundle).unwrap()).unwrap();

    bin()
        .arg("plan")
        .arg(&path)
        .arg("--strict")
        .assert()
        .code(3);
}

#[test]
fn unknown_argument_fails_with_hint() {
    let assert = bin().arg("--frobnicate").assert().failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("unexpected argument"),
        "stderr should explain the bad flag, got: {stderr}"
    );
}

#[test]
fn plan_without_path_fails() {
    let assert = bin().arg("plan").assert().failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(stderr.contains("bundle.json") || stderr.contains("requires"));
}

#[test]
fn missing_bundle_file_fails() {
    let assert = bin()
        .args(["plan", "/no/such/bundle.json"])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(stderr.contains("could not read"));
}

#[test]
fn choosing_both_demo_and_plan_fails() {
    let assert = bin().args(["--demo", "plan", "x.json"]).assert().failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(stderr.contains("exactly one"));
}
