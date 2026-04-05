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
