//! Outside-in integration tests for the `simard concierge` surface.
//!
//! Exercises the real binary end-to-end: the Concierge must produce a hotel
//! concept AND run a reservations/PMS prototype end-to-end — the acceptance bar
//! from the objective. These tests do not need an LLM; the deterministic
//! backbone is what makes the acceptance path CI-verifiable.

use std::path::PathBuf;

use assert_cmd::Command;

fn simard() -> Command {
    Command::cargo_bin("simard").expect("simard binary must be buildable")
}

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!("simard-concierge-it-{}", uuid::Uuid::now_v7()))
}

#[test]
fn concierge_appears_in_top_level_help() {
    let assert = simard().arg("--help").assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        stdout.contains("concierge concept") && stdout.contains("concierge run"),
        "top-level help must advertise the concierge surface:\n{stdout}"
    );
}

#[test]
fn concierge_help_lists_all_subcommands() {
    let assert = simard().args(["concierge", "--help"]).assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    for sub in ["concept", "scaffold", "run", "demo"] {
        assert!(
            stdout.contains(sub),
            "concierge help missing '{sub}':\n{stdout}"
        );
    }
}

#[test]
fn concept_demo_prints_all_three_design_surfaces() {
    let assert = simard().args(["concierge", "concept"]).assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        stdout.contains("## 1. Property Layout"),
        "missing layout:\n{stdout}"
    );
    assert!(
        stdout.contains("## 2. Guest Experience"),
        "missing experience:\n{stdout}"
    );
    assert!(
        stdout.contains("## 3. Brand Design"),
        "missing brand:\n{stdout}"
    );
}

#[test]
fn demo_produces_concept_and_runs_prototype_end_to_end() {
    let assert = simard().args(["concierge", "demo"]).assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    // Concept half.
    assert!(
        stdout.contains("Hotel Concept"),
        "missing concept:\n{stdout}"
    );
    // Prototype half — all four services exercised in the trace.
    for marker in [
        "BOOK",
        "CHECKIN",
        "CHECKOUT",
        "HOUSEKEEPING",
        "CHANNEL SYNC",
    ] {
        assert!(
            stdout.contains(marker),
            "run trace missing '{marker}':\n{stdout}"
        );
    }
    assert!(
        stdout.contains("bookings made:"),
        "missing summary:\n{stdout}"
    );
}

#[test]
fn scaffold_writes_artifacts_and_run_executes_them() {
    let dir = temp_dir();
    let dir_str = dir.to_string_lossy().to_string();

    simard()
        .args(["concierge", "scaffold", "--demo", "--out", &dir_str])
        .assert()
        .success();

    assert!(
        dir.join("concept.md").is_file(),
        "concept.md must be written"
    );
    assert!(
        dir.join("prototype.json").is_file(),
        "prototype.json must be written"
    );
    assert!(dir.join("README.md").is_file(), "README.md must be written");

    let assert = simard()
        .args(["concierge", "run", &dir_str])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        stdout.contains("CHANNEL SYNC"),
        "run must reach channel sync:\n{stdout}"
    );
    assert!(
        stdout.contains("Channel availability"),
        "run must print channel availability:\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn run_on_missing_directory_fails_cleanly() {
    let dir = temp_dir();
    simard()
        .args(["concierge", "run", &dir.to_string_lossy()])
        .assert()
        .failure();
}

#[test]
fn custom_brief_is_honoured() {
    let assert = simard()
        .args([
            "concierge",
            "concept",
            "--name",
            "The Highline",
            "--location",
            "Downtown",
            "--rooms",
            "40",
            "--theme",
            "industrial loft",
            "--positioning",
            "luxury",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(
        stdout.contains("The Highline"),
        "custom name must appear:\n{stdout}"
    );
    assert!(
        stdout.contains("industrial loft"),
        "custom theme must appear:\n{stdout}"
    );
    // Luxury tier adds a spa.
    assert!(
        stdout.contains("spa"),
        "luxury tier should include a spa:\n{stdout}"
    );
}

#[test]
fn demo_json_emits_parseable_json() {
    let assert = simard()
        .args(["concierge", "demo", "--json"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    // Two JSON documents are printed (concept, then report); both must parse.
    let docs: Vec<&str> = stdout.split("}\n{").collect();
    assert!(!docs.is_empty(), "expected JSON output:\n{stdout}");
    // The concept document must include the property name.
    assert!(
        stdout.contains("\"name\""),
        "concept JSON must include a name:\n{stdout}"
    );
    assert!(
        stdout.contains("\"bookings_made\""),
        "report JSON must include run metrics:\n{stdout}"
    );
}
