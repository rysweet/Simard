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

    // ── benchmark_scenarios ─────────────────────────────────────────

    #[test]
    fn benchmark_scenarios_is_not_empty() {
        let scenarios = benchmark_scenarios();
        assert!(
            !scenarios.is_empty(),
            "benchmark_scenarios should return at least one scenario"
        );
    }

    #[test]
    fn benchmark_scenarios_have_non_empty_ids() {
        for scenario in benchmark_scenarios() {
            assert!(!scenario.id.is_empty(), "Scenario id should not be empty");
        }
    }

    #[test]
    fn benchmark_scenarios_have_non_empty_titles() {
        for scenario in benchmark_scenarios() {
            assert!(
                !scenario.title.is_empty(),
                "Scenario {} should have a title",
                scenario.id
            );
        }
    }

    #[test]
    fn benchmark_scenarios_ids_are_unique() {
        let scenarios = benchmark_scenarios();
        let mut ids: Vec<&str> = scenarios.iter().map(|s| s.id).collect();
        let len_before = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), len_before, "Scenario IDs should be unique");
    }

    #[test]
    fn benchmark_scenarios_have_required_fields() {
        for scenario in benchmark_scenarios() {
            assert!(
                !scenario.identity.is_empty(),
                "identity empty for {}",
                scenario.id
            );
            assert!(
                !scenario.base_type.is_empty(),
                "base_type empty for {}",
                scenario.id
            );
            assert!(
                !scenario.objective.is_empty(),
                "objective empty for {}",
                scenario.id
            );
        }
    }

    // ── run_gym_list ────────────────────────────────────────────────

    #[test]
    fn run_gym_list_succeeds() {
        // This function just prints to stdout, so we verify it does not error
        let result = run_gym_list();
        assert!(result.is_ok());
    }
}
