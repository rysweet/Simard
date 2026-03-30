use std::fmt::{self, Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::bootstrap::builtin_base_type_registry_for_manifest;
use crate::error::{SimardError, SimardResult};
use crate::evidence::{EvidenceRecord, EvidenceSource, EvidenceStore, InMemoryEvidenceStore};
use crate::goals::InMemoryGoalStore;
use crate::handoff::RuntimeHandoffSnapshot;
use crate::identity::{
    BuiltinIdentityLoader, IdentityLoadRequest, IdentityLoader, ManifestContract,
};
use crate::memory::{InMemoryMemoryStore, MemoryRecord, MemoryScope, MemoryStore};
use crate::metadata::{Freshness, Provenance};
use crate::prompt_assets::FilePromptAssetStore;
use crate::reflection::ReflectiveRuntime;
use crate::review::{ReviewRequest, ReviewSignal, ReviewTargetKind, build_review_artifact};
use crate::runtime::{
    BaseTypeRegistry, CoordinatedSupervisor, LocalRuntime, LoopbackMailboxTransport,
    LoopbackMeshTopologyDriver, RuntimePorts, RuntimeRequest, RuntimeState, RuntimeTopology,
};
use crate::session::SessionPhase;
use crate::session::UuidSessionIdGenerator;

const STARTER_SUITE_ID: &str = "starter";
const DEFAULT_OUTPUT_ROOT: &str = "target/simard-gym";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BenchmarkClass {
    RepoExploration,
    Documentation,
    SafeCodeChange,
    SessionQuality,
}

impl Display for BenchmarkClass {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::RepoExploration => "repo-exploration",
            Self::Documentation => "documentation",
            Self::SafeCodeChange => "safe-code-change",
            Self::SessionQuality => "session-quality",
        };
        f.write_str(label)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BenchmarkScenario {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub class: BenchmarkClass,
    pub identity: &'static str,
    pub base_type: &'static str,
    pub topology: RuntimeTopology,
    pub objective: &'static str,
    pub expected_min_runtime_evidence: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BenchmarkCheckResult {
    pub id: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BenchmarkArtifactPaths {
    pub run_dir: String,
    pub report_json: String,
    pub report_txt: String,
    pub review_json: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BenchmarkRuntimeReport {
    pub identity: String,
    pub selected_base_type: String,
    pub topology: String,
    pub adapter_implementation: String,
    pub topology_backend: String,
    pub transport_backend: String,
    pub supervisor_backend: String,
    pub runtime_node: String,
    pub mailbox_address: String,
    pub snapshot_state_before_stop: String,
    pub snapshot_state_after_stop: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BenchmarkHandoffReport {
    pub exported_state: String,
    pub exported_memory_records: usize,
    pub exported_evidence_records: usize,
    pub restored_runtime_state: String,
    pub restored_session_phase: Option<String>,
    pub restored_session_objective: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BenchmarkScorecard {
    pub task_completed: bool,
    pub evidence_quality: String,
    pub correctness_checks_passed: usize,
    pub correctness_checks_total: usize,
    pub unnecessary_action_count: Option<u32>,
    pub retry_count: u32,
    pub human_review_notes: Vec<String>,
    pub measurement_notes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BenchmarkRunReport {
    pub suite_id: String,
    pub scenario: BenchmarkScenario,
    pub session_id: String,
    pub run_started_at_unix_ms: u128,
    pub passed: bool,
    pub checks: Vec<BenchmarkCheckResult>,
    pub scorecard: BenchmarkScorecard,
    pub plan: String,
    pub execution_summary: String,
    pub reflection_summary: String,
    pub benchmark_memory_key: String,
    pub benchmark_evidence_id: String,
    pub runtime: BenchmarkRuntimeReport,
    pub handoff: BenchmarkHandoffReport,
    pub artifacts: BenchmarkArtifactPaths,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BenchmarkSuiteScenarioSummary {
    pub scenario_id: String,
    pub passed: bool,
    pub session_id: String,
    pub report_json: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BenchmarkSuiteReport {
    pub suite_id: String,
    pub run_started_at_unix_ms: u128,
    pub passed: bool,
    pub scenarios: Vec<BenchmarkSuiteScenarioSummary>,
    pub artifact_path: String,
}

pub fn benchmark_scenarios() -> Vec<BenchmarkScenario> {
    vec![
        BenchmarkScenario {
            id: "repo-exploration-local",
            title: "Repo exploration on local harness",
            description: "Exercise a bounded repo-exploration task through the gym identity on the single-process local harness.",
            class: BenchmarkClass::RepoExploration,
            identity: "simard-gym",
            base_type: "local-harness",
            topology: RuntimeTopology::SingleProcess,
            objective: "Inspect repository structure, identify likely extension points, and summarize where benchmark and runtime changes should land.",
            expected_min_runtime_evidence: 3,
        },
        BenchmarkScenario {
            id: "docs-refresh-copilot",
            title: "Documentation refresh through copilot-sdk alias",
            description: "Exercise a documentation-oriented benchmark while preserving the explicit copilot-sdk selection and honest local-harness implementation identity.",
            class: BenchmarkClass::Documentation,
            identity: "simard-gym",
            base_type: "copilot-sdk",
            topology: RuntimeTopology::SingleProcess,
            objective: "Produce a concise documentation-oriented execution summary for the current repository state and report the relevant reflected runtime contracts.",
            expected_min_runtime_evidence: 3,
        },
        BenchmarkScenario {
            id: "safe-code-change-rusty-clawd",
            title: "Safe code change style task on rusty-clawd",
            description: "Exercise a bounded safe-change objective on the distinct rusty-clawd backend through the loopback multi-process topology.",
            class: BenchmarkClass::SafeCodeChange,
            identity: "simard-gym",
            base_type: "rusty-clawd",
            topology: RuntimeTopology::MultiProcess,
            objective: "Plan a narrow, reviewable runtime change and summarize the exact evidence an operator would inspect before approving it.",
            expected_min_runtime_evidence: 4,
        },
        BenchmarkScenario {
            id: "composite-session-review",
            title: "Composite identity session quality review",
            description: "Exercise the composite engineer identity as a session-quality benchmark so the starter suite covers the shipped composite identity as well as the dedicated gym identity.",
            class: BenchmarkClass::SessionQuality,
            identity: "simard-composite-engineer",
            base_type: "local-harness",
            topology: RuntimeTopology::SingleProcess,
            objective: "Run a disciplined bounded engineering session, preserve evidence, and produce a concise operator-facing summary of what happened.",
            expected_min_runtime_evidence: 3,
        },
    ]
}

pub fn run_benchmark_scenario(
    scenario_id: &str,
    output_root: impl AsRef<Path>,
) -> SimardResult<BenchmarkRunReport> {
    let scenario = benchmark_scenarios()
        .into_iter()
        .find(|candidate| candidate.id == scenario_id)
        .ok_or_else(|| SimardError::BenchmarkScenarioNotFound {
            scenario_id: scenario_id.to_string(),
        })?;
    execute_scenario(scenario, STARTER_SUITE_ID, output_root.as_ref())
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
    let started_at_unix_ms = now_unix_ms()?;
    let mut scenario_summaries = Vec::new();
    let mut suite_passed = true;

    for scenario in benchmark_scenarios() {
        let report = execute_scenario(scenario, suite_id, output_root)?;
        suite_passed &= report.passed;
        scenario_summaries.push(BenchmarkSuiteScenarioSummary {
            scenario_id: report.scenario.id.to_string(),
            passed: report.passed,
            session_id: report.session_id.clone(),
            report_json: report.artifacts.report_json.clone(),
        });
    }

    let suite_dir = output_root.join("suites");
    create_dir_all(&suite_dir)?;
    let suite_artifact = suite_dir.join(format!("{suite_id}.json"));
    let suite_report = BenchmarkSuiteReport {
        suite_id: suite_id.to_string(),
        run_started_at_unix_ms: started_at_unix_ms,
        passed: suite_passed,
        scenarios: scenario_summaries,
        artifact_path: display_path(&suite_artifact),
    };
    write_json(&suite_artifact, &suite_report)?;
    Ok(suite_report)
}

fn execute_scenario(
    scenario: BenchmarkScenario,
    suite_id: &str,
    output_root: &Path,
) -> SimardResult<BenchmarkRunReport> {
    let started_at_unix_ms = now_unix_ms()?;
    let prompt_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("prompt_assets");
    let prompt_store = Arc::new(FilePromptAssetStore::new(prompt_root));
    let memory_store = Arc::new(InMemoryMemoryStore::try_default()?);
    let evidence_store = Arc::new(InMemoryEvidenceStore::try_default()?);

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
        runtime_ports_for_topology(
            prompt_store,
            Arc::clone(&memory_store),
            Arc::clone(&evidence_store),
            builtin_base_type_registry_for_manifest(&manifest)?,
            scenario.topology,
        )?,
        request.clone(),
    )?;

    runtime.start()?;
    let outcome = runtime.run(scenario.objective.to_string())?;
    let ready_snapshot = runtime.snapshot()?;

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
    })?;

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

    let exported = runtime.export_handoff()?;
    let restored = restore_from_handoff(&manifest, &request, &exported)?;
    let restored_snapshot = restored.snapshot()?;

    runtime.stop()?;
    let stopped_snapshot = runtime.snapshot()?;

    let checks = vec![
        BenchmarkCheckResult {
            id: "session-complete".to_string(),
            passed: outcome.session.phase == SessionPhase::Complete,
            detail: format!(
                "session phase after execution was '{}'",
                outcome.session.phase
            ),
        },
        BenchmarkCheckResult {
            id: "runtime-ready-before-stop".to_string(),
            passed: ready_snapshot.runtime_state == RuntimeState::Ready,
            detail: format!(
                "runtime state before stop was '{}'",
                ready_snapshot.runtime_state
            ),
        },
        BenchmarkCheckResult {
            id: "runtime-stopped-after-stop".to_string(),
            passed: stopped_snapshot.runtime_state == RuntimeState::Stopped,
            detail: format!(
                "runtime state after stop was '{}'",
                stopped_snapshot.runtime_state
            ),
        },
        BenchmarkCheckResult {
            id: "reflection-summary-present".to_string(),
            passed: !outcome.reflection.summary.trim().is_empty(),
            detail: "reflection summary was non-empty".to_string(),
        },
        BenchmarkCheckResult {
            id: "runtime-evidence-produced".to_string(),
            passed: ready_snapshot.evidence_records >= scenario.expected_min_runtime_evidence,
            detail: format!(
                "runtime recorded {} evidence records before benchmark capture; expected at least {}",
                ready_snapshot.evidence_records, scenario.expected_min_runtime_evidence
            ),
        },
        BenchmarkCheckResult {
            id: "exported-benchmark-artifacts".to_string(),
            passed: exported.memory_records.len() >= 3 && exported.evidence_records.len() >= 4,
            detail: format!(
                "exported {} memory records and {} evidence records",
                exported.memory_records.len(),
                exported.evidence_records.len()
            ),
        },
        BenchmarkCheckResult {
            id: "handoff-restores-session-boundary".to_string(),
            passed: restored_snapshot.session_phase == Some(SessionPhase::Complete),
            detail: format!(
                "restored session phase was '{}'",
                restored_snapshot
                    .session_phase
                    .map(|phase| phase.to_string())
                    .unwrap_or_else(|| "<none>".to_string())
            ),
        },
        BenchmarkCheckResult {
            id: "handoff-objective-redacted".to_string(),
            passed: exported
                .session
                .as_ref()
                .map(|session| {
                    session.objective.starts_with("objective-metadata(")
                        && session.objective.ends_with(')')
                })
                .unwrap_or(false),
            detail: exported
                .session
                .as_ref()
                .map(|session| format!("exported session objective was '{}'", session.objective))
                .unwrap_or_else(|| {
                    "exported handoff did not include a session boundary".to_string()
                }),
        },
    ];
    let passed = checks.iter().all(|check| check.passed);
    let run_dir = output_root
        .join(scenario.id)
        .join(outcome.session.id.as_str());
    create_dir_all(&run_dir)?;
    let report_json = run_dir.join("report.json");
    let report_txt = run_dir.join("report.txt");
    let review_json = run_dir.join("review.json");
    let measurement_notes = vec![
        "v1 benchmark foundation derives evidence from runtime, memory, and handoff artifacts rather than a task-specific code-change judge".to_string(),
        "unnecessary_action_count remains unmeasured until the benchmark runner can classify shell/tool actions directly".to_string(),
        "retry_count is currently zero because the benchmark runner does not yet re-plan or retry failed scenarios automatically".to_string(),
    ];
    let review = build_review_artifact(
        ReviewRequest {
            target_kind: ReviewTargetKind::Benchmark,
            target_label: format!("{suite_id}:{}", scenario.id),
            execution_summary: outcome.execution_summary.clone(),
            reflection_summary: outcome.reflection.summary.clone(),
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
        &exported,
    )?;
    let report = BenchmarkRunReport {
        suite_id: suite_id.to_string(),
        scenario,
        session_id: outcome.session.id.to_string(),
        run_started_at_unix_ms: started_at_unix_ms,
        passed,
        scorecard: BenchmarkScorecard {
            task_completed: passed,
            evidence_quality: if exported.evidence_records.len() >= 4 {
                "sufficient".to_string()
            } else {
                "thin".to_string()
            },
            correctness_checks_passed: checks.iter().filter(|check| check.passed).count(),
            correctness_checks_total: checks.len(),
            unnecessary_action_count: None,
            retry_count: 0,
            human_review_notes: review
                .proposals
                .iter()
                .map(|proposal| format!("{}: {}", proposal.title, proposal.suggested_change))
                .collect(),
            measurement_notes,
        },
        checks,
        plan: outcome.plan,
        execution_summary: outcome.execution_summary,
        reflection_summary: outcome.reflection.summary,
        benchmark_memory_key,
        benchmark_evidence_id,
        runtime: BenchmarkRuntimeReport {
            identity: ready_snapshot.identity_name,
            selected_base_type: ready_snapshot.selected_base_type.to_string(),
            topology: ready_snapshot.topology.to_string(),
            adapter_implementation: ready_snapshot.adapter_backend.identity,
            topology_backend: ready_snapshot.topology_backend.identity,
            transport_backend: ready_snapshot.transport_backend.identity,
            supervisor_backend: ready_snapshot.supervisor_backend.identity,
            runtime_node: ready_snapshot.runtime_node.to_string(),
            mailbox_address: ready_snapshot.mailbox_address.to_string(),
            snapshot_state_before_stop: ready_snapshot.runtime_state.to_string(),
            snapshot_state_after_stop: stopped_snapshot.runtime_state.to_string(),
        },
        handoff: BenchmarkHandoffReport {
            exported_state: exported.exported_state.to_string(),
            exported_memory_records: exported.memory_records.len(),
            exported_evidence_records: exported.evidence_records.len(),
            restored_runtime_state: restored_snapshot.runtime_state.to_string(),
            restored_session_phase: restored_snapshot
                .session_phase
                .map(|phase| phase.to_string()),
            restored_session_objective: exported
                .session
                .as_ref()
                .map(|session| session.objective.clone()),
        },
        artifacts: BenchmarkArtifactPaths {
            run_dir: display_path(&run_dir),
            report_json: display_path(&report_json),
            report_txt: display_path(&report_txt),
            review_json: display_path(&review_json),
        },
    };
    write_json(&report_json, &report)?;
    write_text(&report_txt, render_text_report(&report))?;
    write_json(&review_json, &review)?;
    Ok(report)
}

fn restore_from_handoff(
    manifest: &crate::IdentityManifest,
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
        RuntimeTopology::SingleProcess => Ok(RuntimePorts::new(
            prompt_store,
            memory_store,
            evidence_store,
            base_types,
            Arc::new(UuidSessionIdGenerator),
        )),
        RuntimeTopology::MultiProcess | RuntimeTopology::Distributed => {
            Ok(RuntimePorts::with_runtime_services(
                prompt_store,
                memory_store,
                evidence_store,
                Arc::new(InMemoryGoalStore::try_default()?),
                base_types,
                Arc::new(LoopbackMeshTopologyDriver::try_default()?),
                Arc::new(LoopbackMailboxTransport::try_default()?),
                Arc::new(CoordinatedSupervisor::try_default()?),
                Arc::new(UuidSessionIdGenerator),
            ))
        }
    }
}

fn render_text_report(report: &BenchmarkRunReport) -> String {
    let mut lines = vec![
        format!("Suite: {}", report.suite_id),
        format!(
            "Scenario: {} ({})",
            report.scenario.id, report.scenario.title
        ),
        format!("Passed: {}", report.passed),
        format!("Identity: {}", report.runtime.identity),
        format!("Base type: {}", report.runtime.selected_base_type),
        format!("Topology: {}", report.runtime.topology),
        format!(
            "Checks passed: {}/{}",
            report.scorecard.correctness_checks_passed, report.scorecard.correctness_checks_total
        ),
        format!("Plan: {}", report.plan),
        format!("Execution summary: {}", report.execution_summary),
        format!("Reflection summary: {}", report.reflection_summary),
        format!("Review artifact: {}", report.artifacts.review_json),
        "Checks:".to_string(),
    ];
    for check in &report.checks {
        lines.push(format!(
            "- {}: {} ({})",
            check.id,
            if check.passed { "passed" } else { "failed" },
            check.detail
        ));
    }
    if !report.scorecard.human_review_notes.is_empty() {
        lines.push("Human review notes:".to_string());
        for note in &report.scorecard.human_review_notes {
            lines.push(format!("- {note}"));
        }
    }
    lines.join("\n")
}

fn create_dir_all(path: &Path) -> SimardResult<()> {
    fs::create_dir_all(path).map_err(|error| SimardError::ArtifactIo {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

fn write_json<T>(path: &Path, value: &T) -> SimardResult<()>
where
    T: Serialize,
{
    let json = serde_json::to_string_pretty(value).map_err(|error| SimardError::ArtifactIo {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    write_text(path, format!("{json}\n"))
}

fn write_text(path: &Path, contents: String) -> SimardResult<()> {
    fs::write(path, contents).map_err(|error| SimardError::ArtifactIo {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn now_unix_ms() -> SimardResult<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| SimardError::ClockBeforeUnixEpoch {
            reason: error.to_string(),
        })?
        .as_millis())
}

pub fn default_output_root() -> PathBuf {
    PathBuf::from(DEFAULT_OUTPUT_ROOT)
}
