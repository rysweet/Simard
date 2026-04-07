use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::bootstrap::builtin_base_type_registry_for_manifest;
use crate::error::SimardResult;
use crate::evidence::{EvidenceRecord, EvidenceSource, EvidenceStore, InMemoryEvidenceStore};
use crate::identity::{
    BuiltinIdentityLoader, IdentityLoadRequest, IdentityLoader, ManifestContract,
};
use crate::memory::{InMemoryMemoryStore, MemoryRecord, MemoryScope, MemoryStore};
use crate::metadata::{Freshness, Provenance};
use crate::prompt_assets::FilePromptAssetStore;
use crate::reflection::ReflectiveRuntime;
use crate::review::{ReviewRequest, ReviewSignal, ReviewTargetKind, build_review_artifact};
use crate::runtime::{LocalRuntime, RuntimeRequest, RuntimeState};
use crate::session::SessionPhase;

use super::reporting;
use super::scenarios;
use super::types::{
    BenchmarkArtifactPaths, BenchmarkCheckResult, BenchmarkHandoffReport, BenchmarkRunReport,
    BenchmarkRuntimeReport, BenchmarkScenario, BenchmarkScorecard,
};

// Re-export metric types so `use super::executor::X` in tests still works.
pub(super) use super::executor_metrics::*;

pub(super) fn execute_scenario(
    scenario: BenchmarkScenario,
    suite_id: &str,
    output_root: &Path,
) -> SimardResult<BenchmarkRunReport> {
    let started_at_unix_ms = reporting::now_unix_ms()?;
    let (runtime_artifacts, metric_facts) = run_scenario_runtime(&scenario, suite_id)?;
    let checks = build_scenario_checks(&scenario, &runtime_artifacts);
    let passed = checks.iter().all(|check| check.passed);
    build_and_write_report(
        &scenario,
        suite_id,
        output_root,
        started_at_unix_ms,
        runtime_artifacts,
        metric_facts,
        checks,
        passed,
    )
}

struct RuntimeArtifacts {
    outcome: crate::runtime::SessionOutcome,
    ready_snapshot: crate::reflection::ReflectionSnapshot,
    stopped_snapshot: crate::reflection::ReflectionSnapshot,
    exported: crate::handoff::RuntimeHandoffSnapshot,
    restored_snapshot: crate::reflection::ReflectionSnapshot,
    benchmark_memory_key: String,
    benchmark_evidence_id: String,
}

fn run_scenario_runtime(
    scenario: &BenchmarkScenario,
    suite_id: &str,
) -> SimardResult<(RuntimeArtifacts, BenchmarkMetricFacts)> {
    let prompt_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets");
    let prompt_store = Arc::new(FilePromptAssetStore::new(prompt_root));
    let memory_store = Arc::new(InMemoryMemoryStore::try_default()?);
    let evidence_store = Arc::new(InMemoryEvidenceStore::try_default()?);
    let mut metric_facts = BenchmarkMetricFacts::default();

    let contract = ManifestContract::new(
        "simard::gym::run_benchmark_scenario",
        "simard-gym-cli -> identity-loader -> runtime-ports -> local-runtime",
        vec![
            format!("suite:{suite_id}"),
            format!("scenario:{}", scenario.id),
            format!("identity:{}", scenario.identity),
            format!("base-type:{}", scenario.base_type),
            format!("topology:{}", scenario.topology),
        ],
        Provenance::new("benchmark-gym", format!("simard-gym:{}", scenario.id)),
        Freshness::now()?,
    )?;
    let manifest = BuiltinIdentityLoader.load(&IdentityLoadRequest::new(
        scenario.identity,
        env!("CARGO_PKG_VERSION"),
        contract,
    ))?;
    let request = RuntimeRequest::new(
        manifest.clone(),
        crate::BaseTypeId::new(scenario.base_type),
        scenario.topology,
    );
    let mut runtime = LocalRuntime::compose(
        super::runtime_ports_for_topology(
            prompt_store,
            Arc::clone(&memory_store),
            Arc::clone(&evidence_store),
            builtin_base_type_registry_for_manifest(&manifest)?,
            scenario.topology,
        )?,
        request.clone(),
    )?;

    runtime.start()?;
    metric_facts.record_required_action();
    let outcome = runtime.run(scenario.objective.to_string())?;
    metric_facts.record_primary_attempt();
    metric_facts.record_required_action();
    let ready_snapshot = runtime.snapshot()?;
    metric_facts.record_required_action();

    let benchmark_memory_key = format!("{}-benchmark-summary", outcome.session.id);
    memory_store.put(MemoryRecord {
        key: benchmark_memory_key.clone(),
        scope: MemoryScope::Benchmark,
        value: format!(
            "suite={suite_id}; scenario={}; class={}; identity={}; base_type={}; topology={}",
            scenario.id, scenario.class, scenario.identity, scenario.base_type, scenario.topology
        ),
        session_id: outcome.session.id.clone(),
        recorded_in: SessionPhase::Complete,
        created_at: None,
    })?;
    metric_facts.record_required_action();

    let benchmark_evidence_id = format!("{}-benchmark-capture", outcome.session.id);
    evidence_store.record(EvidenceRecord {
        id: benchmark_evidence_id.clone(),
        session_id: outcome.session.id.clone(),
        phase: SessionPhase::Complete,
        detail: format!(
            "benchmark-scenario={} suite={} identity={} base_type={} topology={}",
            scenario.id, suite_id, scenario.identity, scenario.base_type, scenario.topology
        ),
        source: EvidenceSource::Runtime,
    })?;
    metric_facts.record_required_action();

    let exported = runtime.export_handoff()?;
    metric_facts.record_required_action();
    let restored = super::restore_from_handoff(&manifest, &request, &exported)?;
    metric_facts.record_required_action();
    let restored_snapshot = restored.snapshot()?;
    metric_facts.record_required_action();

    runtime.stop()?;
    metric_facts.record_required_action();
    let stopped_snapshot = runtime.snapshot()?;
    metric_facts.record_required_action();

    Ok((
        RuntimeArtifacts {
            outcome,
            ready_snapshot,
            stopped_snapshot,
            exported,
            restored_snapshot,
            benchmark_memory_key,
            benchmark_evidence_id,
        },
        metric_facts,
    ))
}

fn build_scenario_checks(
    scenario: &BenchmarkScenario,
    arts: &RuntimeArtifacts,
) -> Vec<BenchmarkCheckResult> {
    let core_checks = vec![
        BenchmarkCheckResult {
            id: "session-complete".to_string(),
            passed: arts.outcome.session.phase == SessionPhase::Complete,
            detail: format!(
                "session phase after execution was '{}'",
                arts.outcome.session.phase
            ),
        },
        BenchmarkCheckResult {
            id: "runtime-ready-before-stop".to_string(),
            passed: arts.ready_snapshot.runtime_state == RuntimeState::Ready,
            detail: format!(
                "runtime state before stop was '{}'",
                arts.ready_snapshot.runtime_state
            ),
        },
        BenchmarkCheckResult {
            id: "runtime-stopped-after-stop".to_string(),
            passed: arts.stopped_snapshot.runtime_state == RuntimeState::Stopped,
            detail: format!(
                "runtime state after stop was '{}'",
                arts.stopped_snapshot.runtime_state
            ),
        },
        BenchmarkCheckResult {
            id: "reflection-summary-present".to_string(),
            passed: !arts.outcome.reflection.summary.trim().is_empty(),
            detail: "reflection summary was non-empty".to_string(),
        },
        BenchmarkCheckResult {
            id: "runtime-evidence-produced".to_string(),
            passed: arts.ready_snapshot.evidence_records >= scenario.expected_min_runtime_evidence,
            detail: format!(
                "runtime recorded {} evidence records before benchmark capture; expected at least {}",
                arts.ready_snapshot.evidence_records, scenario.expected_min_runtime_evidence
            ),
        },
        BenchmarkCheckResult {
            id: "exported-benchmark-artifacts".to_string(),
            passed: arts.exported.memory_records.len() >= 3
                && arts.exported.evidence_records.len() >= 4,
            detail: format!(
                "exported {} memory records and {} evidence records",
                arts.exported.memory_records.len(),
                arts.exported.evidence_records.len()
            ),
        },
        BenchmarkCheckResult {
            id: "handoff-restores-session-boundary".to_string(),
            passed: arts.restored_snapshot.session_phase == Some(SessionPhase::Complete),
            detail: format!(
                "restored session phase was '{}'",
                arts.restored_snapshot
                    .session_phase
                    .map(|phase| phase.to_string())
                    .unwrap_or_else(|| "<none>".to_string())
            ),
        },
        BenchmarkCheckResult {
            id: "handoff-objective-redacted".to_string(),
            passed: arts
                .exported
                .session
                .as_ref()
                .map(|session| {
                    session.objective.starts_with("objective-metadata(")
                        && session.objective.ends_with(')')
                })
                .unwrap_or(false),
            detail: arts
                .exported
                .session
                .as_ref()
                .map(|session| format!("exported session objective was '{}'", session.objective))
                .unwrap_or_else(|| {
                    "exported handoff did not include a session boundary".to_string()
                }),
        },
    ];
    let class_checks = scenarios::class_specific_checks(scenario, &arts.outcome, &arts.exported);
    [core_checks, class_checks].concat()
}

#[allow(clippy::too_many_arguments)]
fn build_and_write_report(
    scenario: &BenchmarkScenario,
    suite_id: &str,
    output_root: &Path,
    started_at_unix_ms: u128,
    arts: RuntimeArtifacts,
    metric_facts: BenchmarkMetricFacts,
    checks: Vec<BenchmarkCheckResult>,
    passed: bool,
) -> SimardResult<BenchmarkRunReport> {
    let run_dir = output_root
        .join(scenario.id)
        .join(arts.outcome.session.id.as_str());
    reporting::create_dir_all(&run_dir)?;
    let report_json = run_dir.join("report.json");
    let report_txt = run_dir.join("report.txt");
    let review_json = run_dir.join("review.json");
    let derived_metrics = derive_benchmark_metrics(&metric_facts);
    let measurement_notes = vec![
        "v1 benchmark foundation derives evidence from runtime, memory, and handoff artifacts rather than a task-specific code-change judge".to_string(),
        "Attempt and action metrics derive from benchmark-controlled gym-runner facts only; they intentionally do not classify arbitrary adapter-level subcommands inside the scenario objective.".to_string(),
    ];
    let review = build_review_artifact(
        ReviewRequest {
            target_kind: ReviewTargetKind::Benchmark,
            target_label: format!("{suite_id}:{}", scenario.id),
            execution_summary: arts.outcome.execution_summary.clone(),
            reflection_summary: arts.outcome.reflection.summary.clone(),
            measurement_notes: measurement_notes.clone(),
            signals: checks
                .iter()
                .map(|check| ReviewSignal {
                    id: check.id.clone(),
                    passed: check.passed,
                    detail: check.detail.clone(),
                })
                .collect(),
        },
        &arts.exported,
    )?;
    let report = BenchmarkRunReport {
        suite_id: suite_id.to_string(),
        scenario: *scenario,
        session_id: arts.outcome.session.id.to_string(),
        run_started_at_unix_ms: started_at_unix_ms,
        passed,
        scorecard: BenchmarkScorecard {
            task_completed: passed,
            evidence_quality: if arts.exported.evidence_records.len() >= 4 {
                "sufficient".to_string()
            } else {
                "thin".to_string()
            },
            correctness_checks_passed: checks.iter().filter(|check| check.passed).count(),
            correctness_checks_total: checks.len(),
            unnecessary_action_count: derived_metrics.unnecessary_action_count,
            retry_count: derived_metrics.retry_count,
            human_review_notes: review
                .proposals
                .iter()
                .map(|proposal| format!("{}: {}", proposal.title, proposal.suggested_change))
                .collect(),
            measurement_notes,
        },
        checks,
        plan: arts.outcome.plan,
        execution_summary: arts.outcome.execution_summary,
        reflection_summary: arts.outcome.reflection.summary,
        benchmark_memory_key: arts.benchmark_memory_key,
        benchmark_evidence_id: arts.benchmark_evidence_id,
        runtime: BenchmarkRuntimeReport {
            identity: arts.ready_snapshot.identity_name,
            selected_base_type: arts.ready_snapshot.selected_base_type.to_string(),
            topology: arts.ready_snapshot.topology.to_string(),
            adapter_implementation: arts.ready_snapshot.adapter_backend.identity,
            topology_backend: arts.ready_snapshot.topology_backend.identity,
            transport_backend: arts.ready_snapshot.transport_backend.identity,
            supervisor_backend: arts.ready_snapshot.supervisor_backend.identity,
            runtime_node: arts.ready_snapshot.runtime_node.to_string(),
            mailbox_address: arts.ready_snapshot.mailbox_address.to_string(),
            snapshot_state_before_stop: arts.ready_snapshot.runtime_state.to_string(),
            snapshot_state_after_stop: arts.stopped_snapshot.runtime_state.to_string(),
        },
        handoff: BenchmarkHandoffReport {
            exported_state: arts.exported.exported_state.to_string(),
            exported_memory_records: arts.exported.memory_records.len(),
            exported_evidence_records: arts.exported.evidence_records.len(),
            restored_runtime_state: arts.restored_snapshot.runtime_state.to_string(),
            restored_session_phase: arts
                .restored_snapshot
                .session_phase
                .map(|phase| phase.to_string()),
            restored_session_objective: arts
                .exported
                .session
                .as_ref()
                .map(|session| session.objective.clone()),
        },
        artifacts: BenchmarkArtifactPaths {
            run_dir: reporting::display_path(&run_dir),
            report_json: reporting::display_path(&report_json),
            report_txt: reporting::display_path(&report_txt),
            review_json: reporting::display_path(&review_json),
        },
    };
    reporting::write_json(&report_json, &report)?;
    reporting::write_text(&report_txt, reporting::render_text_report(&report))?;
    reporting::write_json(&review_json, &review)?;
    Ok(report)
}
