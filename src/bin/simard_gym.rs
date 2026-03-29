use simard::{
    benchmark_scenarios, default_output_root, run_benchmark_scenario, run_benchmark_suite,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or(usage())?;

    match command.as_str() {
        "list" => {
            println!("Simard benchmark scenarios:");
            for scenario in benchmark_scenarios() {
                println!(
                    "- {} | class={} | identity={} | base_type={} | topology={}",
                    scenario.id,
                    scenario.class,
                    scenario.identity,
                    scenario.base_type,
                    scenario.topology
                );
                println!("  {}", scenario.title);
            }
        }
        "run" => {
            let scenario_id = args.next().ok_or("expected scenario id")?;
            let report = run_benchmark_scenario(&scenario_id, default_output_root())?;
            println!("Scenario: {}", report.scenario.id);
            println!("Suite: {}", report.suite_id);
            println!("Session: {}", report.session_id);
            println!("Passed: {}", report.passed);
            println!(
                "Checks passed: {}/{}",
                report.scorecard.correctness_checks_passed,
                report.scorecard.correctness_checks_total
            );
            println!("Artifact report: {}", report.artifacts.report_json);
            println!("Artifact summary: {}", report.artifacts.report_txt);
        }
        "run-suite" => {
            let suite_id = args.next().ok_or("expected suite id")?;
            let report = run_benchmark_suite(&suite_id, default_output_root())?;
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
        }
        _ => return Err(usage().into()),
    }

    Ok(())
}

fn usage() -> &'static str {
    "usage: simard-gym <list|run <scenario-id>|run-suite <suite-id>>"
}
