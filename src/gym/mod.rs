mod executor;
mod executor_metrics;
mod reporting;
mod scenarios;
mod types;

#[cfg(test)]
mod tests_executor;
#[cfg(test)]
mod tests_executor_metrics;
#[cfg(test)]
mod tests_gym_extra;
#[cfg(test)]
mod tests_mod;
#[cfg(test)]
mod tests_reporting;
#[cfg(test)]
mod tests_scenarios;
#[cfg(test)]
mod tests_types;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::bootstrap::builtin_base_type_registry_for_manifest;
use crate::error::{SimardError, SimardResult};
use crate::evidence::InMemoryEvidenceStore;
use crate::goals::InMemoryGoalStore;
use crate::handoff::RuntimeHandoffSnapshot;
use crate::identity::IdentityManifest;
use crate::memory::InMemoryMemoryStore;
use crate::prompt_assets::FilePromptAssetStore;
use crate::runtime::{
    BaseTypeRegistry, CoordinatedSupervisor, LocalRuntime, LoopbackMailboxTransport,
    LoopbackMeshTopologyDriver, RuntimePorts, RuntimeRequest, RuntimeTopology,
};
use crate::session::UuidSessionIdGenerator;

pub(crate) use reporting::{render_benchmark_count, render_benchmark_delta};
pub use scenarios::benchmark_scenarios;
pub use types::{
    BenchmarkArtifactPaths, BenchmarkCheckResult, BenchmarkClass, BenchmarkComparisonArtifactPaths,
    BenchmarkComparisonDelta, BenchmarkComparisonReport, BenchmarkComparisonRunSummary,
    BenchmarkComparisonStatus, BenchmarkHandoffReport, BenchmarkRunReport, BenchmarkRuntimeReport,
    BenchmarkScenario, BenchmarkScorecard, BenchmarkSuiteReport, BenchmarkSuiteScenarioSummary,
};

const STARTER_SUITE_ID: &str = "starter";
const DEFAULT_OUTPUT_ROOT: &str = "target/simard-gym";

pub fn run_benchmark_scenario(
    scenario_id: &str,
    output_root: impl AsRef<Path>,
) -> SimardResult<BenchmarkRunReport> {
    let scenario = scenarios::resolve_benchmark_scenario(scenario_id)?;
    executor::execute_scenario(scenario, STARTER_SUITE_ID, output_root.as_ref())
}

pub fn run_benchmark_suite(
    suite_id: &str,
    output_root: impl AsRef<Path>,
) -> SimardResult<BenchmarkSuiteReport> {
    if suite_id != STARTER_SUITE_ID {
        return Err(SimardError::BenchmarkSuiteNotFound {
            suite_id: suite_id.to_string(),
        });
    }

    let output_root = output_root.as_ref();
    let started_at_unix_ms = reporting::now_unix_ms()?;
    let mut scenario_summaries = Vec::new();
    let mut suite_passed = true;

    for scenario in benchmark_scenarios().iter().copied() {
        let report = executor::execute_scenario(scenario, suite_id, output_root)?;
        suite_passed &= report.passed;
        scenario_summaries.push(BenchmarkSuiteScenarioSummary {
            scenario_id: report.scenario.id.to_string(),
            passed: report.passed,
            session_id: report.session_id.clone(),
            report_json: report.artifacts.report_json.clone(),
        });
    }

    let suite_dir = output_root.join("suites");
    reporting::create_dir_all(&suite_dir)?;
    let suite_artifact = suite_dir.join(format!("{suite_id}.json"));
    let suite_report = BenchmarkSuiteReport {
        suite_id: suite_id.to_string(),
        run_started_at_unix_ms: started_at_unix_ms,
        passed: suite_passed,
        scenarios: scenario_summaries,
        artifact_path: reporting::display_path(&suite_artifact),
    };
    reporting::write_json(&suite_artifact, &suite_report)?;
    Ok(suite_report)
}

pub fn compare_latest_benchmark_runs(
    scenario_id: &str,
    output_root: impl AsRef<Path>,
) -> SimardResult<BenchmarkComparisonReport> {
    let scenario = scenarios::resolve_benchmark_scenario(scenario_id)?;
    let output_root = output_root.as_ref();
    let mut reports = reporting::load_scenario_run_reports(scenario.id, output_root)?;
    if reports.len() < 2 {
        return Err(SimardError::BenchmarkComparisonUnavailable {
            scenario_id: scenario.id.to_string(),
            reason: format!(
                "need at least two completed runs under '{}'",
                reporting::display_path(&output_root.join(scenario.id))
            ),
        });
    }
    reports.sort_by_key(|entry| {
        (
            entry.report.run_started_at_unix_ms,
            entry.report.session_id.as_str().to_owned(),
        )
    });
    let current = reports.pop().expect("checked length >= 2");
    let previous = reports.pop().expect("checked length >= 2");

    let current_summary = reporting::summarize_stored_run(&current);
    let previous_summary = reporting::summarize_stored_run(&previous);
    let delta = BenchmarkComparisonDelta {
        correctness_checks_passed: current_summary.correctness_checks_passed as i64
            - previous_summary.correctness_checks_passed as i64,
        unnecessary_action_count: reporting::benchmark_count_delta(
            current_summary.unnecessary_action_count,
            previous_summary.unnecessary_action_count,
        ),
        retry_count: reporting::benchmark_count_delta(
            current_summary.retry_count,
            previous_summary.retry_count,
        ),
        exported_memory_records: current_summary.exported_memory_records as i64
            - previous_summary.exported_memory_records as i64,
        exported_evidence_records: current_summary.exported_evidence_records as i64
            - previous_summary.exported_evidence_records as i64,
    };
    let status = reporting::compare_runs(&current_summary, &previous_summary);
    let summary =
        reporting::render_comparison_summary(status, &current_summary, &previous_summary, &delta);

    let comparison_dir = output_root
        .join("comparisons")
        .join(scenario.id)
        .join(format!(
            "{}-vs-{}",
            current_summary.session_id, previous_summary.session_id
        ));
    reporting::create_dir_all(&comparison_dir)?;
    let report_json = comparison_dir.join("report.json");
    let report_txt = comparison_dir.join("report.txt");
    let report = BenchmarkComparisonReport {
        scenario_id: current.report.scenario.id,
        scenario_title: current.report.scenario.title,
        status,
        summary,
        current: current_summary,
        previous: previous_summary,
        delta,
        artifact_paths: BenchmarkComparisonArtifactPaths {
            report_json: reporting::display_path(&report_json),
            report_txt: reporting::display_path(&report_txt),
        },
    };
    reporting::write_json(&report_json, &report)?;
    reporting::write_text(
        &report_txt,
        reporting::render_text_comparison_report(&report),
    )?;
    Ok(report)
}

fn restore_from_handoff(
    manifest: &IdentityManifest,
    request: &RuntimeRequest,
    exported: &RuntimeHandoffSnapshot,
) -> SimardResult<LocalRuntime> {
    let prompt_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets");
    let prompt_store = Arc::new(FilePromptAssetStore::new(prompt_root));
    let memory_store = Arc::new(InMemoryMemoryStore::try_default()?);
    let evidence_store = Arc::new(InMemoryEvidenceStore::try_default()?);
    LocalRuntime::compose_from_handoff(
        runtime_ports_for_topology(
            prompt_store,
            memory_store,
            evidence_store,
            builtin_base_type_registry_for_manifest(manifest)?,
            request.topology,
        )?,
        request.clone(),
        exported.clone(),
    )
}

fn runtime_ports_for_topology(
    prompt_store: Arc<FilePromptAssetStore>,
    memory_store: Arc<InMemoryMemoryStore>,
    evidence_store: Arc<InMemoryEvidenceStore>,
    base_types: BaseTypeRegistry,
    topology: RuntimeTopology,
) -> SimardResult<RuntimePorts> {
    match topology {
        RuntimeTopology::SingleProcess => RuntimePorts::new(
            prompt_store,
            memory_store,
            evidence_store,
            base_types,
            Arc::new(UuidSessionIdGenerator),
        ),
        RuntimeTopology::MultiProcess | RuntimeTopology::Distributed => {
            RuntimePorts::with_runtime_services(
                prompt_store,
                memory_store,
                evidence_store,
                Arc::new(InMemoryGoalStore::try_default()?),
                base_types,
                Arc::new(LoopbackMeshTopologyDriver::try_default()?),
                Arc::new(LoopbackMailboxTransport::try_default()?),
                Arc::new(CoordinatedSupervisor::try_default()?),
                Arc::new(UuidSessionIdGenerator),
            )
        }
    }
}

pub fn default_output_root() -> PathBuf {
    PathBuf::from(DEFAULT_OUTPUT_ROOT)
}
