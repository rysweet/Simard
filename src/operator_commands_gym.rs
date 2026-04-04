use crate::operator_commands::{print_display, print_text};
use crate::{
    benchmark_scenarios, compare_latest_benchmark_runs, default_output_root,
    run_benchmark_scenario, run_benchmark_suite,
};

pub fn run_gym_list() -> Result<(), Box<dyn std::error::Error>> {
    println!("Simard benchmark scenarios:");
    for scenario in benchmark_scenarios() {
        println!(
            "- {} | class={} | identity={} | base_type={} | topology={}",
            scenario.id, scenario.class, scenario.identity, scenario.base_type, scenario.topology
        );
        println!("  {}", scenario.title);
    }
    Ok(())
}

pub fn run_gym_scenario(scenario_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let report = run_benchmark_scenario(scenario_id, default_output_root())?;
    print_text("Scenario", report.scenario.id);
    print_text("Suite", &report.suite_id);
    print_text("Session", &report.session_id);
    print_display("Passed", report.passed);
    print_display(
        "Checks passed",
        format!(
            "{}/{}",
            report.scorecard.correctness_checks_passed, report.scorecard.correctness_checks_total
        ),
    );
    print_display(
        "Unnecessary actions",
        crate::gym::render_benchmark_count(report.scorecard.unnecessary_action_count),
    );
    print_display(
        "Retry count",
        crate::gym::render_benchmark_count(report.scorecard.retry_count),
    );
    print_text("Artifact report", &report.artifacts.report_json);
    print_text("Artifact summary", &report.artifacts.report_txt);
    print_text("Review artifact", &report.artifacts.review_json);
    Ok(())
}

pub fn run_gym_compare(scenario_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let report = compare_latest_benchmark_runs(scenario_id, default_output_root())?;
    print_text("Scenario", &report.scenario_id);
    print_display("Comparison status", report.status);
    print_text("Comparison summary", &report.summary);
    print_text("Current session", &report.current.session_id);
    print_display("Current passed", report.current.passed);
    print_display(
        "Current checks passed",
        format!(
            "{}/{}",
            report.current.correctness_checks_passed, report.current.correctness_checks_total
        ),
    );
    print_text("Current report", &report.current.report_json);
    print_display(
        "Current unnecessary actions",
        crate::gym::render_benchmark_count(report.current.unnecessary_action_count),
    );
    print_display(
        "Current retry count",
        crate::gym::render_benchmark_count(report.current.retry_count),
    );
    print_text("Previous session", &report.previous.session_id);
    print_display("Previous passed", report.previous.passed);
    print_display(
        "Previous checks passed",
        format!(
            "{}/{}",
            report.previous.correctness_checks_passed, report.previous.correctness_checks_total
        ),
    );
    print_text("Previous report", &report.previous.report_json);
    print_display(
        "Previous unnecessary actions",
        crate::gym::render_benchmark_count(report.previous.unnecessary_action_count),
    );
    print_display(
        "Previous retry count",
        crate::gym::render_benchmark_count(report.previous.retry_count),
    );
    print_display(
        "Delta correctness checks passed",
        format!("{:+}", report.delta.correctness_checks_passed),
    );
    print_display(
        "Delta unnecessary actions",
        crate::gym::render_benchmark_delta(report.delta.unnecessary_action_count),
    );
    print_display(
        "Delta retry count",
        crate::gym::render_benchmark_delta(report.delta.retry_count),
    );
    print_display(
        "Delta exported memory records",
        format!("{:+}", report.delta.exported_memory_records),
    );
    print_display(
        "Delta exported evidence records",
        format!("{:+}", report.delta.exported_evidence_records),
    );
    print_text(
        "Comparison artifact report",
        &report.artifact_paths.report_json,
    );
    print_text(
        "Comparison artifact summary",
        &report.artifact_paths.report_txt,
    );
    Ok(())
}

pub fn run_gym_suite(suite_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let report = run_benchmark_suite(suite_id, default_output_root())?;
    println!("Suite: {}", report.suite_id);
    println!("Suite passed: {}", report.passed);
    for scenario in &report.scenarios {
        println!(
            "- {}: {} ({})",
            scenario.scenario_id,
            if scenario.passed { "passed" } else { "failed" },
            scenario.report_json
        );
    }
    println!("Suite artifact report: {}", report.artifact_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gym_list_succeeds() {
        let result = run_gym_list();
        assert!(result.is_ok());
    }

    #[test]
    fn benchmark_scenarios_not_empty() {
        let scenarios = benchmark_scenarios();
        assert!(
            !scenarios.is_empty(),
            "benchmark_scenarios should return at least one scenario"
        );
    }

    #[test]
    fn benchmark_scenarios_have_required_fields() {
        for scenario in benchmark_scenarios() {
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
        let scenarios = benchmark_scenarios();
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
        for scenario in benchmark_scenarios() {
            // BenchmarkClass is an enum — just verify Display works
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
        for scenario in benchmark_scenarios() {
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
        let root = default_output_root();
        // Should return a valid PathBuf (may or may not exist)
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
}
