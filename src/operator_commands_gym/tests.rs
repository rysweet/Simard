use super::*;

#[test]
fn gym_list_succeeds() {
    let result = run_gym_list();
    assert!(result.is_ok());
}

#[test]
fn benchmark_scenarios_not_empty() {
    let scenarios = crate::benchmark_scenarios();
    assert!(
        !scenarios.is_empty(),
        "benchmark_scenarios should return at least one scenario"
    );
}

#[test]
fn benchmark_scenarios_have_required_fields() {
    for scenario in crate::benchmark_scenarios() {
        assert!(!scenario.id.is_empty(), "scenario id must not be empty");
        assert!(
            !scenario.title.is_empty(),
            "scenario title must not be empty"
        );
        assert!(
            !scenario.identity.is_empty(),
            "scenario identity must not be empty"
        );
        assert!(
            !scenario.base_type.is_empty(),
            "scenario base_type must not be empty"
        );
    }
}

#[test]
fn benchmark_scenarios_have_unique_ids() {
    let scenarios = crate::benchmark_scenarios();
    let mut ids: Vec<&str> = scenarios.iter().map(|s| s.id).collect();
    let original_count = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        original_count,
        "benchmark scenario ids must be unique"
    );
}

#[test]
fn render_benchmark_count_some() {
    assert_eq!(crate::gym::render_benchmark_count(Some(5)), "5");
}

#[test]
fn render_benchmark_count_zero() {
    assert_eq!(crate::gym::render_benchmark_count(Some(0)), "0");
}

#[test]
fn render_benchmark_count_none() {
    assert_eq!(crate::gym::render_benchmark_count(None), "unmeasured");
}

#[test]
fn render_benchmark_delta_positive() {
    let result = crate::gym::render_benchmark_delta(Some(3));
    assert_eq!(result, "+3");
}

#[test]
fn render_benchmark_delta_negative() {
    let result = crate::gym::render_benchmark_delta(Some(-2));
    assert_eq!(result, "-2");
}

#[test]
fn render_benchmark_delta_zero() {
    let result = crate::gym::render_benchmark_delta(Some(0));
    assert_eq!(result, "+0");
}

#[test]
fn render_benchmark_delta_none() {
    assert_eq!(crate::gym::render_benchmark_delta(None), "unmeasured");
}

#[test]
fn gym_scenario_errors_with_invalid_id() {
    let result = run_gym_scenario("nonexistent-scenario-id-12345");
    assert!(result.is_err());
}

#[test]
fn gym_compare_errors_with_invalid_id() {
    let result = run_gym_compare("nonexistent-scenario-id-12345");
    assert!(result.is_err());
}

#[test]
fn gym_suite_errors_with_invalid_id() {
    let result = run_gym_suite("nonexistent-suite-id-12345");
    assert!(result.is_err());
}

#[test]
fn benchmark_scenarios_class_is_valid() {
    for scenario in crate::benchmark_scenarios() {
        let class_str = format!("{}", scenario.class);
        assert!(
            !class_str.is_empty(),
            "scenario class display must not be empty for {}",
            scenario.id
        );
    }
}

#[test]
fn benchmark_scenarios_topology_is_valid() {
    for scenario in crate::benchmark_scenarios() {
        let topology_str = format!("{}", scenario.topology);
        assert!(
            !topology_str.is_empty(),
            "scenario topology display must not be empty for {}",
            scenario.id
        );
    }
}

#[test]
fn gym_scenario_error_message_is_descriptive() {
    let result = run_gym_scenario("totally-fake-scenario");
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("totally-fake-scenario")
            || msg.contains("not registered")
            || msg.contains("not found"),
        "error should be descriptive: {msg}"
    );
}

#[test]
fn gym_compare_error_message_is_descriptive() {
    let result = run_gym_compare("totally-fake-scenario");
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(!msg.is_empty(), "error message should not be empty");
}

#[test]
fn gym_suite_error_message_is_descriptive() {
    let result = run_gym_suite("totally-fake-suite");
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(!msg.is_empty(), "error message should not be empty");
}

#[test]
fn default_output_root_returns_path() {
    let root = crate::default_output_root();
    assert!(
        !root.as_os_str().is_empty(),
        "output root should not be empty"
    );
}

#[test]
fn render_benchmark_count_large_value() {
    assert_eq!(crate::gym::render_benchmark_count(Some(999999)), "999999");
}

#[test]
fn render_benchmark_delta_large_positive() {
    assert_eq!(crate::gym::render_benchmark_delta(Some(100)), "+100");
}

#[test]
fn render_benchmark_delta_large_negative() {
    assert_eq!(crate::gym::render_benchmark_delta(Some(-100)), "-100");
}

#[test]
fn benchmark_scenarios_description_not_empty() {
    for scenario in crate::benchmark_scenarios() {
        assert!(
            !scenario.description.is_empty(),
            "scenario description must not be empty for {}",
            scenario.id
        );
    }
}

#[test]
fn benchmark_scenarios_objective_not_empty() {
    for scenario in crate::benchmark_scenarios() {
        assert!(
            !scenario.objective.is_empty(),
            "scenario objective must not be empty for {}",
            scenario.id
        );
    }
}

#[test]
fn benchmark_scenarios_topology_is_known_variant() {
    for scenario in crate::benchmark_scenarios() {
        match scenario.topology {
            crate::runtime::RuntimeTopology::SingleProcess
            | crate::runtime::RuntimeTopology::MultiProcess
            | crate::runtime::RuntimeTopology::Distributed => {}
        }
    }
}

#[test]
fn benchmark_scenarios_min_evidence_is_reasonable() {
    for scenario in crate::benchmark_scenarios() {
        assert!(
            scenario.expected_min_runtime_evidence <= 100,
            "min evidence for {} seems too high: {}",
            scenario.id,
            scenario.expected_min_runtime_evidence
        );
    }
}

#[test]
fn render_benchmark_count_one() {
    assert_eq!(crate::gym::render_benchmark_count(Some(1)), "1");
}

#[test]
fn render_benchmark_delta_one() {
    assert_eq!(crate::gym::render_benchmark_delta(Some(1)), "+1");
}

#[test]
fn render_benchmark_delta_minus_one() {
    assert_eq!(crate::gym::render_benchmark_delta(Some(-1)), "-1");
}

#[test]
fn render_benchmark_count_u32_max() {
    assert_eq!(
        crate::gym::render_benchmark_count(Some(u32::MAX)),
        format!("{}", u32::MAX)
    );
}

#[test]
fn gym_list_returns_ok() {
    assert!(run_gym_list().is_ok());
}

#[test]
fn gym_scenario_distinct_error_for_each_bad_id() {
    let r1 = run_gym_scenario("bad-id-alpha");
    let r2 = run_gym_scenario("bad-id-beta");
    assert!(r1.is_err());
    assert!(r2.is_err());
    let m1 = r1.unwrap_err().to_string();
    let m2 = r2.unwrap_err().to_string();
    assert_ne!(m1, m2, "different IDs should produce different errors");
}

#[test]
fn default_output_root_is_relative() {
    assert!(crate::default_output_root().is_relative());
}

// ── run_gym_compare success path ────────────────────────────────────
//
// `run_gym_compare` resolves a real scenario, loads the two most-recent
// stored run reports from `default_output_root()/<scenario>/<run>/report.json`,
// renders a comparison, and prints every summary/delta field. The error
// branch (fewer than two runs / unknown id) is already covered above; these
// tests cover the success branch hermetically by seeding two report fixtures
// on disk — no network, no sleeps, no live runtime, and no cognitive-memory
// writes (the comparison path only touches plain JSON artifacts).

fn seed_run_report(
    run_dir: &std::path::Path,
    scenario_id: &str,
    session_id: &str,
    run_started_at_unix_ms: u64,
    passed: bool,
    checks_passed: usize,
) {
    std::fs::create_dir_all(run_dir).expect("create run fixture dir");
    let report = serde_json::json!({
        "suite_id": "starter",
        "scenario": { "id": scenario_id, "title": "Compare-coverage fixture scenario" },
        "session_id": session_id,
        "run_started_at_unix_ms": run_started_at_unix_ms,
        "passed": passed,
        "scorecard": {
            "correctness_checks_passed": checks_passed,
            "correctness_checks_total": 3,
            "evidence_quality": "sufficient",
            "unnecessary_action_count": 0,
            "retry_count": 0
        },
        "handoff": {
            "exported_memory_records": 2,
            "exported_evidence_records": 2
        }
    });
    std::fs::write(
        run_dir.join("report.json"),
        serde_json::to_string_pretty(&report).expect("serialize report fixture"),
    )
    .expect("write report.json fixture");
}

/// Seed exactly two run-report fixtures under `default_output_root()` for the
/// scenario at `scenario_index`, returning the scenario id and its directory.
/// The directory is removed first so the fixture set is deterministic.
fn seed_two_runs(
    scenario_index: usize,
    older_passed: bool,
    older_checks: usize,
    newer_passed: bool,
    newer_checks: usize,
) -> (&'static str, std::path::PathBuf) {
    let scenario_id = crate::benchmark_scenarios()
        .get(scenario_index)
        .expect("scenario index within registered scenarios")
        .id;
    let scenario_dir = crate::default_output_root().join(scenario_id);
    let _ = std::fs::remove_dir_all(&scenario_dir);
    seed_run_report(
        &scenario_dir.join("run-older"),
        scenario_id,
        "session-older",
        1_000,
        older_passed,
        older_checks,
    );
    seed_run_report(
        &scenario_dir.join("run-newer"),
        scenario_id,
        "session-newer",
        2_000,
        newer_passed,
        newer_checks,
    );
    (scenario_id, scenario_dir)
}

fn cleanup_compare_fixtures(scenario_id: &str, scenario_dir: &std::path::Path) {
    let _ = std::fs::remove_dir_all(scenario_dir);
    let _ = std::fs::remove_dir_all(
        crate::default_output_root()
            .join("comparisons")
            .join(scenario_id),
    );
}

#[test]
fn gym_compare_succeeds_with_two_seeded_runs_improved() {
    // Scenario index 1 keeps this isolated from the other compare test and
    // from the run-scenario test (which owns index 0). `run_gym_compare` only
    // resolves the id and reads the seeded fixtures, so the scenario's base
    // type is irrelevant here.
    let (scenario_id, scenario_dir) = seed_two_runs(1, false, 1, true, 3);

    let result = run_gym_compare(scenario_id);

    cleanup_compare_fixtures(scenario_id, &scenario_dir);
    assert!(
        result.is_ok(),
        "compare should succeed with two seeded runs: {result:?}"
    );
}

#[test]
fn gym_compare_succeeds_with_two_seeded_runs_unchanged() {
    // Identical metrics across both runs exercise the "unchanged" comparison
    // branch while still driving every print line in run_gym_compare.
    let (scenario_id, scenario_dir) = seed_two_runs(2, true, 3, true, 3);

    let result = run_gym_compare(scenario_id);

    cleanup_compare_fixtures(scenario_id, &scenario_dir);
    assert!(
        result.is_ok(),
        "compare should succeed for two identical seeded runs: {result:?}"
    );
}

// ── run_gym_scenario success path ───────────────────────────────────
//
// `run_gym_scenario` runs a benchmark scenario end-to-end and prints the
// resulting report. The error branch (unknown id) is covered above. This
// test covers the success branch hermetically using the single-process
// `local-harness` scenario `repo-exploration-local` — the same scenario the
// existing `tests/review.rs` suite drives through `run_benchmark_scenario`.
// The harness runs entirely in-process with `InMemory*` stores (no network,
// no external services, no cognitive-memory writes); artifacts land under the
// crate-relative `default_output_root()` and are cleaned up afterward.

#[test]
fn gym_scenario_succeeds_for_local_harness_scenario() {
    // `repo-exploration-local` is the single-process local-harness scenario
    // that `tests/review.rs` also drives through `run_benchmark_scenario`; it
    // is registered first and is owned exclusively by this test (the compare
    // tests use scenarios 1 and 2), so there is no fixture-directory race.
    let scenario_id = "repo-exploration-local";
    let scenario_dir = crate::default_output_root().join(scenario_id);
    let _ = std::fs::remove_dir_all(&scenario_dir);

    let result = run_gym_scenario(scenario_id);

    let _ = std::fs::remove_dir_all(&scenario_dir);
    assert!(
        result.is_ok(),
        "run_gym_scenario should succeed for the local-harness scenario: {result:?}"
    );
}
