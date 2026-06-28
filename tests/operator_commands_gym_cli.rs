//! Integration tests for the `operator_commands_gym` surface.
//!
//! These exercise the public gym command API (`run_gym_compare`,
//! `run_gym_list`) and the legacy CLI dispatcher (`dispatch_legacy_gym_cli`)
//! end-to-end against seeded on-disk fixtures — no network, no sleeps, no
//! live runtime/bridge state, and no cognitive-memory writes. The success
//! branch of `run_gym_compare` is the largest uncovered region in
//! `src/operator_commands_gym/commands.rs`; driving it through both the
//! library entry point and the CLI dispatcher mirrors the deterministic
//! CLI-surface pattern used for the `bin` group.
//!
//! Filed against rysweet/Simard#1752 (test-coverage: raise
//! operator_commands_gym from 43% to 70%); parent #1735.

use std::path::Path;

/// A registered scenario id at offset `from_end` counting back from the last
/// scenario. Using ids far from the start keeps these tests from racing with
/// the in-crate unit tests (which own the first three scenarios), and using a
/// *distinct* id per test keeps the two compare tests in this binary from
/// racing with each other on the same `target/simard-gym/<scenario>` dir.
fn scenario_id_from_end(from_end: usize) -> &'static str {
    let scenarios = simard::benchmark_scenarios();
    let len = scenarios.len();
    assert!(
        len > from_end + 3,
        "need enough registered scenarios to pick distinct, non-overlapping ids"
    );
    scenarios[len - 1 - from_end].id
}

fn seed_run_report(
    run_dir: &Path,
    scenario_id: &str,
    session_id: &str,
    run_started_at_unix_ms: u64,
    passed: bool,
    checks_passed: usize,
) {
    std::fs::create_dir_all(run_dir).expect("create run fixture dir");
    let report = serde_json::json!({
        "suite_id": "starter",
        "scenario": { "id": scenario_id, "title": "Integration compare fixture" },
        "session_id": session_id,
        "run_started_at_unix_ms": run_started_at_unix_ms,
        "passed": passed,
        "scorecard": {
            "correctness_checks_passed": checks_passed,
            "correctness_checks_total": 3,
            "evidence_quality": "sufficient",
            "unnecessary_action_count": 1,
            "retry_count": 0
        },
        "handoff": {
            "exported_memory_records": 3,
            "exported_evidence_records": 4
        }
    });
    std::fs::write(
        run_dir.join("report.json"),
        serde_json::to_string_pretty(&report).expect("serialize report fixture"),
    )
    .expect("write report.json fixture");
}

fn seed_two_runs(scenario_id: &str) -> std::path::PathBuf {
    let scenario_dir = simard::default_output_root().join(scenario_id);
    let _ = std::fs::remove_dir_all(&scenario_dir);
    seed_run_report(
        &scenario_dir.join("run-older"),
        scenario_id,
        "session-int-older",
        10_000,
        true,
        2,
    );
    seed_run_report(
        &scenario_dir.join("run-newer"),
        scenario_id,
        "session-int-newer",
        20_000,
        true,
        3,
    );
    scenario_dir
}

fn cleanup(scenario_id: &str, scenario_dir: &Path) {
    let _ = std::fs::remove_dir_all(scenario_dir);
    let _ = std::fs::remove_dir_all(
        simard::default_output_root()
            .join("comparisons")
            .join(scenario_id),
    );
}

#[test]
fn run_gym_compare_succeeds_via_library_entry_point() {
    let scenario_id = scenario_id_from_end(0);
    let scenario_dir = seed_two_runs(scenario_id);

    let result = simard::run_gym_compare(scenario_id);

    cleanup(scenario_id, &scenario_dir);
    assert!(
        result.is_ok(),
        "run_gym_compare should succeed with two seeded runs: {result:?}"
    );
}

#[test]
fn dispatch_compare_subcommand_succeeds_with_seeded_runs() {
    let scenario_id = scenario_id_from_end(1);
    let scenario_dir = seed_two_runs(scenario_id);

    let result = simard::dispatch_legacy_gym_cli(["compare".to_string(), scenario_id.to_string()]);

    cleanup(scenario_id, &scenario_dir);
    assert!(
        result.is_ok(),
        "`simard-gym compare <scenario>` should succeed with two seeded runs: {result:?}"
    );
}

#[test]
fn dispatch_list_subcommand_succeeds() {
    // `list` is pure stdout enumeration of the registered scenarios.
    let result = simard::dispatch_legacy_gym_cli(["list".to_string()]);
    assert!(
        result.is_ok(),
        "`simard-gym list` should succeed: {result:?}"
    );
}

#[test]
fn dispatch_compare_unknown_scenario_is_rejected() {
    let result = simard::dispatch_legacy_gym_cli([
        "compare".to_string(),
        "definitely-not-a-registered-scenario".to_string(),
    ]);
    assert!(
        result.is_err(),
        "unknown scenario id should produce an error"
    );
}
