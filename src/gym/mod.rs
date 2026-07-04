mod executor;
mod executor_metrics;
mod reporting;
mod scenarios;
mod types;

#[cfg(test)]
mod tests_executor;
#[cfg(test)]
mod tests_executor_extra;
#[cfg(test)]
mod tests_executor_metrics;
#[cfg(test)]
mod tests_gym_extra;
#[cfg(test)]
mod tests_mod;
#[cfg(test)]
mod tests_reporting;
#[cfg(test)]
mod tests_reporting_extra;
#[cfg(test)]
mod tests_reporting_more;
#[cfg(test)]
mod tests_scenarios;
#[cfg(test)]
mod tests_scenarios_2;
#[cfg(test)]
mod tests_types;
#[cfg(test)]
mod tests_types_extra;

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

/// Base types that execute deterministically at self-test gate time without
/// external credentials. The self-test / self-update health gate only runs
/// scenarios on these backends so its result never depends on network auth.
const GATE_BASE_TYPES: [&str; 2] = ["local-harness", "terminal-shell"];

/// Returns `true` when `scenario` belongs in the `starter` suite, which is the
/// deterministic health gate run by `simard self-test` and the `self-update`
/// relaunch check (issue #2548).
///
/// The gate must be *genuinely green on a healthy binary*, so it only runs
/// scenarios whose correctness is a property of the binary's own runtime
/// machinery rather than the reasoning quality of an external LLM backend:
///
/// * `SessionQuality` scenarios are graded by backend-agnostic *structural*
///   checks (session lifecycle, memory-export completeness, PTY driving) that a
///   deterministic base type satisfies on every run.
/// * `RepoExploration`, `Documentation`, and `SafeCodeChange` scenarios are
///   graded by *content* checks that scan the agent's prose for domain keywords
///   (e.g. "cargo.toml", "///", "derive"). The deterministic `local-harness`
///   executor returns a fixed template summary and cannot satisfy them, so those
///   scenarios are benchmarks for capable reasoning backends — run them with
///   `simard gym run <scenario-id>` — not health-gate checks.
///
/// Keeping the content-check scenarios out of the gate (rather than weakening
/// their checks) is what lets `self-test` report an honest, deterministic
/// pass/fail instead of a false-green. The full scenario catalogue is unchanged
/// and still surfaced by `gym list` and runnable individually via `gym run`.
fn is_starter_gate_scenario(scenario: &BenchmarkScenario) -> bool {
    matches!(scenario.class, BenchmarkClass::SessionQuality)
        && GATE_BASE_TYPES.contains(&scenario.base_type)
}

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

    for scenario in benchmark_scenarios()
        .iter()
        .copied()
        .filter(is_starter_gate_scenario)
    {
        match executor::execute_scenario(scenario, suite_id, output_root) {
            Ok(report) => {
                suite_passed &= report.passed;
                scenario_summaries.push(BenchmarkSuiteScenarioSummary {
                    scenario_id: report.scenario.id.to_string(),
                    passed: report.passed,
                    skipped: false,
                    skip_reason: None,
                    session_id: report.session_id.clone(),
                    report_json: report.artifacts.report_json.clone(),
                });
            }
            Err(e) => match gate_prerequisite_skip(&e, scenario.base_type) {
                Some(reason) => {
                    eprintln!(
                        "WARN: skipping scenario '{}' (base_type={}): {reason}",
                        scenario.id, scenario.base_type,
                    );
                    scenario_summaries.push(BenchmarkSuiteScenarioSummary {
                        scenario_id: scenario.id.to_string(),
                        passed: false,
                        skipped: true,
                        skip_reason: Some(reason),
                        session_id: String::new(),
                        report_json: String::new(),
                    });
                }
                None => return Err(e),
            },
        }
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

/// Returns `true` when `err` is an auth-related invocation failure for a
/// base type that requires external credentials (e.g. `rusty-clawd` needs
/// Copilot auth). Such errors are expected on stock dev machines and should
/// be skipped rather than failing the entire suite (issue #1743).
fn is_skippable_auth_error(err: &SimardError, base_type: &str) -> bool {
    // Only skip for base types known to require external auth.
    if base_type == "local-harness" {
        return false;
    }
    match err {
        SimardError::AdapterInvocationFailed { reason, .. } => {
            reason.contains("authentication")
                || reason.contains("requires auth")
                || reason.contains("new_copilot")
        }
        // AdapterNotRegistered also fires when the base-type factory cannot
        // instantiate because the auth subsystem is absent.
        SimardError::AdapterNotRegistered { .. } => true,
        _ => false,
    }
}

/// Signature of the launch-failure message emitted by the `terminal-shell`
/// base type when its PTY launcher (`script`) cannot be spawned — see
/// `PtyTerminalSession::launch` ("failed to launch local PTY shell via
/// '<launcher>': <error>"). Matching this specific prefix keeps the skip
/// deliberately narrow: only a *missing / unspawnable* launcher counts as an
/// unavailable prerequisite. A launcher that spawns but then yields wrong
/// output or a non-zero exit surfaces through a different path and remains a
/// genuine, non-skippable failure — skipping that would reintroduce the very
/// false-green issue #2548 fixes.
const PTY_LAUNCH_FAILURE_SIGNATURE: &str = "failed to launch local PTY shell via";

/// Returns `true` when `err` is a `terminal-shell` failure caused by the PTY
/// launcher being unavailable on the host.
///
/// The `interactive-terminal-driving` gate scenario spawns `script` to allocate
/// its *own* PTY, so it runs fine when the parent process has no controlling
/// terminal (exactly how `self-update` invokes `self-test` head-less). But on a
/// host where the launcher is entirely absent (no `script` on `PATH`, or a
/// sandbox with no PTY support), that scenario cannot launch at all. Treating
/// that as an unavailable *environment prerequisite* — rather than a defect in
/// the binary — lets a genuinely healthy binary still self-test green on
/// no-PTY hosts, mirroring the auth-unavailable skip (Pillar 11: honest
/// degradation beats a false-RED).
fn is_skippable_pty_unavailable(err: &SimardError, base_type: &str) -> bool {
    if base_type != "terminal-shell" {
        return false;
    }
    matches!(
        err,
        SimardError::AdapterInvocationFailed { reason, .. }
            if reason.contains(PTY_LAUNCH_FAILURE_SIGNATURE)
    )
}

/// Classifies a gate-scenario failure as a skippable *environment prerequisite*
/// (returning the operator-facing skip reason) or as a genuine failure
/// (returning `None`, so the health gate fails honestly).
///
/// Two prerequisites are recognised, both of which are properties of the *host*
/// rather than defects in the binary under test:
///
/// * external **auth** for credentialed backends (issue #1743), and
/// * a usable **PTY launcher** for the `terminal-shell` base type on a host
///   with no PTY support (issue #2548 / no-PTY hosts).
fn gate_prerequisite_skip(err: &SimardError, base_type: &str) -> Option<String> {
    if is_skippable_auth_error(err, base_type) {
        return Some(format!(
            "backend '{base_type}' requires authentication unavailable at gate-time"
        ));
    }
    if is_skippable_pty_unavailable(err, base_type) {
        return Some(format!(
            "base type '{base_type}' requires a PTY launcher unavailable at gate-time"
        ));
    }
    None
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
