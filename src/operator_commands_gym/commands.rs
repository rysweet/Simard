use crate::operator_commands::{print_display, print_text};
use crate::{
    BenchmarkSuiteReport, benchmark_scenarios, compare_latest_benchmark_runs, default_output_root,
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
        if scenario.skipped {
            let reason = scenario
                .skip_reason
                .as_deref()
                .unwrap_or("auth unavailable");
            println!("- {}: SKIPPED ({})", scenario.scenario_id, reason);
        } else {
            println!(
                "- {}: {} ({})",
                scenario.scenario_id,
                if scenario.passed { "passed" } else { "failed" },
                scenario.report_json
            );
        }
    }
    let skipped_count = report.scenarios.iter().filter(|s| s.skipped).count();
    if skipped_count > 0 {
        println!(
            "WARN: {} scenario(s) skipped due to unavailable auth",
            skipped_count
        );
    }
    println!("Suite artifact report: {}", report.artifact_path);
    // A failing suite MUST surface as a non-zero process exit. `simard self-test`
    // and the `self-update` relaunch gate shell out to `gym run-suite starter`
    // and branch on its exit code; returning `Ok(())` here regardless of
    // `report.passed` is the false-green root cause (issue #2548).
    evaluate_suite_result(&report)
}

/// Run the fixed recall-precision benchmark, append one comparable score to the
/// shared gym history, and print the score plus the gym signal.
///
/// Issue #2491 / #2494 (G1 hybrid measurement): the operator on-ramp to the
/// benchmark rail. Reuses the same [`crate::gym_history::default_db_path`] the
/// OODA gym step and the correlation endpoint use, so all three share one score
/// history and a benchmark score written here is the exact score the dashboard
/// correlation reads back.
pub fn run_gym_recall_precision() -> Result<(), Box<dyn std::error::Error>> {
    use crate::cognitive_memory::recall_precision_bench::{
        recall_precision_corpus_size, run_recall_precision_bench,
    };
    use crate::gym_history::{ScoreHistory, default_db_path, generate_signals};

    let history = ScoreHistory::open(default_db_path())?;
    let commit = Some(env!("SIMARD_GIT_HASH").to_string());
    let record = run_recall_precision_bench(&history, commit)?;
    // The gym signal needs a prior run to compare against; on the first run the
    // scenario has a single record and `generate_signals` yields nothing for it.
    let signal = generate_signals(&history, &record.suite_id)?
        .into_iter()
        .find(|s| s.scenario_id == record.scenario_id)
        .map(|s| s.signal.to_string())
        .unwrap_or_else(|| "stable".to_string());
    println!(
        "{}/{}: score={:.4} signal={} samples={}",
        record.suite_id,
        record.scenario_id,
        record.score,
        signal,
        recall_precision_corpus_size(),
    );
    Ok(())
}

/// Run the fixed reliability-gate benchmark, append one comparable score to the
/// shared gym history, and print the score plus the gym signal.
///
/// Perpetual-cognition goal (reasoner reliability): the operator on-ramp to the
/// reliability-gate benchmark rail. Mirrors [`run_gym_recall_precision`] — reuses
/// the same [`crate::gym_history::default_db_path`] the OODA gym step and the
/// dashboard correlation use, so a benchmark score written here is the exact
/// score those read back, and the score flows through the same
/// [`crate::gym_history::generate_signals`] regression machinery. The benchmark
/// scores a frozen, rubric-labeled corpus through the SAME
/// `fact_reliability::fact_passes_gate` the live write boundaries use, so its
/// accuracy is `1.0` on a correct gate and a drop signals a discrimination
/// regression.
pub fn run_gym_reliability_gate() -> Result<(), Box<dyn std::error::Error>> {
    use crate::fact_reliability_bench::{reliability_gate_corpus_size, run_reliability_gate_bench};
    use crate::gym_history::{ScoreHistory, default_db_path, generate_signals};

    let history = ScoreHistory::open(default_db_path())?;
    let commit = Some(env!("SIMARD_GIT_HASH").to_string());
    let record = run_reliability_gate_bench(&history, commit)?;
    // The gym signal needs a prior run to compare against; on the first run the
    // scenario has a single record and `generate_signals` yields nothing for it.
    let signal = generate_signals(&history, &record.suite_id)?
        .into_iter()
        .find(|s| s.scenario_id == record.scenario_id)
        .map(|s| s.signal.to_string())
        .unwrap_or_else(|| "stable".to_string());
    println!(
        "{}/{}: score={:.4} signal={} samples={}",
        record.suite_id,
        record.scenario_id,
        record.score,
        signal,
        reliability_gate_corpus_size(),
    );
    Ok(())
}

/// Run the enrichment ablation (issue #2942): the hard proof that recalled
/// memory *influences* a decision. Seeds a hermetic in-memory cognitive store
/// with representative facts/procedures, renders one decision WITH recall
/// injected vs WITHOUT (recall suppressed), prints the reproducible delta +
/// verdict, and feeds `delta_bytes` into the hybrid self-measurement (#2644) as
/// `enrichment_ablation_delta`. Deterministic — no daemon, no network.
pub fn run_gym_enrichment_ablation() -> Result<(), Box<dyn std::error::Error>> {
    use serde_json::json;

    use crate::enrichment_observability::{record_ablation_feed, run_enrichment_ablation};
    use crate::memory_client::CognitiveMemoryClient;
    use crate::rpc::RpcErrorPayload;
    use crate::rpc_transport::InMemoryRpcTransport;

    const OBJECTIVE: &str = "implement error handling for the recall pipeline";

    // A seeded, hermetic store: representative facts + procedures for any query.
    let transport =
        InMemoryRpcTransport::new("enrichment-ablation", |method, params| match method {
            "memory.search_facts" => {
                let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
                Ok(json!({"facts": [
                    {"node_id": "sem_001", "concept": "error-handling",
                     "content": format!("prefer typed errors over panics when handling '{query}'"),
                     "confidence": 0.9, "source_id": "src_1", "tags": ["rust"]},
                    {"node_id": "sem_002", "concept": "recall",
                     "content": "recalled facts belong in the prompt preamble, not the objective",
                     "confidence": 0.85, "source_id": "src_2", "tags": ["memory"]},
                    {"node_id": "sem_003", "concept": "observability",
                     "content": "a degrade must fail loud, never silent",
                     "confidence": 0.8, "source_id": "src_3", "tags": ["ops"]}
                ]}))
            }
            "memory.recall_procedure" => Ok(json!({"procedures": [
                {"node_id": "proc_001", "name": "build-and-test",
                 "steps": ["cargo build", "cargo test"],
                 "prerequisites": ["rust toolchain"], "usage_count": 5},
                {"node_id": "proc_002", "name": "verify-recall",
                 "steps": ["seed store", "run ablation", "assert delta>0"],
                 "prerequisites": [], "usage_count": 2}
            ]})),
            "memory.search_episodes_by_keywords" => Ok(json!({"episodes": []})),
            _ => Err(RpcErrorPayload {
                code: -32601,
                message: format!("unknown method: {method}"),
            }),
        });
    let memory = CognitiveMemoryClient::new(Box::new(transport));

    let outcome = run_enrichment_ablation(OBJECTIVE, &memory)?;
    println!(
        "cognition/enrichment_ablation: recall_on_bytes={} recall_off_bytes={} \
         delta_bytes={} facts={} procedures={} preambles_differ={} verdict={}",
        outcome.recall_on_bytes,
        outcome.recall_off_bytes,
        outcome.delta_bytes,
        outcome.facts,
        outcome.procedures,
        outcome.preambles_differ,
        outcome.verdict.as_str(),
    );

    // Feed the hybrid self-measurement (#2644). Best-effort: a metrics-write
    // failure is surfaced loudly but never fails the eval.
    if let Err(e) = record_ablation_feed(&outcome) {
        tracing::warn!(
            target: "simard::enrichment",
            error = %e,
            "failed to record enrichment_ablation_delta (ablation result unaffected)",
        );
    }
    Ok(())
}

/// Convert a completed suite report into a process-level result.
///
/// Returns `Ok(())` for a passing suite and an `Err` (which the CLI turns into a
/// non-zero exit) for a failing one. Skipped scenarios (e.g. auth unavailable)
/// are not failures — the suite's `passed` flag already excludes them — so the
/// error message lists only the scenarios that actually ran and failed.
fn evaluate_suite_result(report: &BenchmarkSuiteReport) -> Result<(), Box<dyn std::error::Error>> {
    if report.passed {
        return Ok(());
    }
    let failed: Vec<&str> = report
        .scenarios
        .iter()
        .filter(|scenario| !scenario.skipped && !scenario.passed)
        .map(|scenario| scenario.scenario_id.as_str())
        .collect();
    let detail = if failed.is_empty() {
        "no scenario-level failures were recorded".to_string()
    } else {
        format!("{} scenario(s) failed: {}", failed.len(), failed.join(", "))
    };
    Err(format!("gym suite '{}' did not pass ({detail})", report.suite_id).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BenchmarkSuiteScenarioSummary;

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

    // ── evaluate_suite_result (issue #2548) ─────────────────────────

    fn scenario_summary(id: &str, passed: bool, skipped: bool) -> BenchmarkSuiteScenarioSummary {
        BenchmarkSuiteScenarioSummary {
            scenario_id: id.to_string(),
            passed,
            skipped,
            skip_reason: skipped.then(|| "auth unavailable".to_string()),
            session_id: format!("session-{id}"),
            report_json: format!("target/simard-gym/{id}/report.json"),
        }
    }

    fn suite_report(
        passed: bool,
        scenarios: Vec<BenchmarkSuiteScenarioSummary>,
    ) -> BenchmarkSuiteReport {
        BenchmarkSuiteReport {
            suite_id: "starter".to_string(),
            run_started_at_unix_ms: 0,
            passed,
            scenarios,
            artifact_path: "target/simard-gym/suites/starter.json".to_string(),
        }
    }

    #[test]
    fn evaluate_suite_result_ok_when_passed() {
        let report = suite_report(true, vec![scenario_summary("a", true, false)]);
        assert!(evaluate_suite_result(&report).is_ok());
    }

    #[test]
    fn evaluate_suite_result_err_when_failed_names_failing_scenarios() {
        let report = suite_report(
            false,
            vec![
                scenario_summary("passing-one", true, false),
                scenario_summary("failing-one", false, false),
            ],
        );
        let err = evaluate_suite_result(&report).expect_err("failing suite must be an error");
        let msg = err.to_string();
        assert!(
            msg.contains("starter"),
            "message should name the suite: {msg}"
        );
        assert!(
            msg.contains("failing-one"),
            "message should name the failing scenario: {msg}"
        );
        assert!(
            !msg.contains("passing-one"),
            "message should not list passing scenarios: {msg}"
        );
    }

    #[test]
    fn evaluate_suite_result_err_ignores_skipped_scenarios() {
        let report = suite_report(
            false,
            vec![
                scenario_summary("skipped-one", false, true),
                scenario_summary("failing-one", false, false),
            ],
        );
        let msg = evaluate_suite_result(&report)
            .expect_err("failing suite must be an error")
            .to_string();
        assert!(
            !msg.contains("skipped-one"),
            "skipped scenarios are not failures: {msg}"
        );
        assert!(
            msg.contains("failing-one"),
            "should list real failures: {msg}"
        );
    }
}
