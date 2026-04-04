use std::fmt::{self, Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

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
    TestWriting,
}

impl Display for BenchmarkClass {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::RepoExploration => "repo-exploration",
            Self::Documentation => "documentation",
            Self::SafeCodeChange => "safe-code-change",
            Self::SessionQuality => "session-quality",
            Self::TestWriting => "test-writing",
        };
        f.write_str(label)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
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
    pub retry_count: Option<u32>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BenchmarkComparisonStatus {
    Improved,
    Unchanged,
    Regressed,
}

impl Display for BenchmarkComparisonStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Improved => "improved",
            Self::Unchanged => "unchanged",
            Self::Regressed => "regressed",
        };
        f.write_str(label)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BenchmarkComparisonRunSummary {
    pub suite_id: String,
    pub session_id: String,
    pub run_started_at_unix_ms: u128,
    pub passed: bool,
    pub correctness_checks_passed: usize,
    pub correctness_checks_total: usize,
    pub evidence_quality: String,
    pub unnecessary_action_count: Option<u32>,
    pub retry_count: Option<u32>,
    pub exported_memory_records: usize,
    pub exported_evidence_records: usize,
    pub report_json: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BenchmarkComparisonDelta {
    pub correctness_checks_passed: i64,
    pub unnecessary_action_count: Option<i64>,
    pub retry_count: Option<i64>,
    pub exported_memory_records: i64,
    pub exported_evidence_records: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BenchmarkComparisonArtifactPaths {
    pub report_json: String,
    pub report_txt: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BenchmarkComparisonReport {
    pub scenario_id: String,
    pub scenario_title: String,
    pub status: BenchmarkComparisonStatus,
    pub summary: String,
    pub current: BenchmarkComparisonRunSummary,
    pub previous: BenchmarkComparisonRunSummary,
    pub delta: BenchmarkComparisonDelta,
    pub artifact_paths: BenchmarkComparisonArtifactPaths,
}

#[derive(Clone, Debug, Deserialize)]
struct StoredBenchmarkScenario {
    id: String,
    title: String,
}

#[derive(Clone, Debug, Deserialize)]
struct StoredBenchmarkScorecard {
    correctness_checks_passed: usize,
    correctness_checks_total: usize,
    evidence_quality: String,
    #[serde(default)]
    unnecessary_action_count: Option<u32>,
    #[serde(default)]
    retry_count: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
struct StoredBenchmarkHandoffReport {
    exported_memory_records: usize,
    exported_evidence_records: usize,
}

#[derive(Clone, Debug, Deserialize)]
struct StoredBenchmarkRunReport {
    suite_id: String,
    scenario: StoredBenchmarkScenario,
    session_id: String,
    run_started_at_unix_ms: u128,
    passed: bool,
    scorecard: StoredBenchmarkScorecard,
    handoff: StoredBenchmarkHandoffReport,
}

#[derive(Clone, Debug)]
struct StoredBenchmarkRunArtifact {
    report_path: PathBuf,
    report: StoredBenchmarkRunReport,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BenchmarkAttemptClassification {
    Primary,
    Retry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BenchmarkAttemptFact {
    classification: Option<BenchmarkAttemptClassification>,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BenchmarkActionClassification {
    Required,
    Unnecessary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BenchmarkActionFact {
    classification: Option<BenchmarkActionClassification>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct BenchmarkMetricFacts {
    attempts: Vec<BenchmarkAttemptFact>,
    actions: Vec<BenchmarkActionFact>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DerivedBenchmarkMetrics {
    unnecessary_action_count: Option<u32>,
    retry_count: Option<u32>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl BenchmarkMetricFacts {
    fn record_primary_attempt(&mut self) {
        self.attempts.push(BenchmarkAttemptFact {
            classification: Some(BenchmarkAttemptClassification::Primary),
        });
    }

    fn record_retry_attempt(&mut self) {
        self.attempts.push(BenchmarkAttemptFact {
            classification: Some(BenchmarkAttemptClassification::Retry),
        });
    }

    fn record_unmeasured_attempt(&mut self) {
        self.attempts.push(BenchmarkAttemptFact {
            classification: None,
        });
    }

    fn record_required_action(&mut self) {
        self.actions.push(BenchmarkActionFact {
            classification: Some(BenchmarkActionClassification::Required),
        });
    }

    fn record_unnecessary_action(&mut self) {
        self.actions.push(BenchmarkActionFact {
            classification: Some(BenchmarkActionClassification::Unnecessary),
        });
    }

    fn record_unmeasured_action(&mut self) {
        self.actions.push(BenchmarkActionFact {
            classification: None,
        });
    }
}

const BENCHMARK_SCENARIOS: [BenchmarkScenario; 9] = [
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
    BenchmarkScenario {
        id: "interactive-terminal-driving",
        title: "Interactive terminal driving on terminal-shell",
        description: "Exercise the engineer identity through the terminal-shell base type by launching a bounded interactive child process, waiting for prompts, and sending follow-up inputs like an operator validating generic PTY-driven control flow.",
        class: BenchmarkClass::SessionQuality,
        identity: "simard-engineer",
        base_type: "terminal-shell",
        topology: RuntimeTopology::SingleProcess,
        objective: "working-directory: .\ncommand: sh -c 'printf \"terminal-ready\\n\"; while IFS= read -r line; do if [ \"$line\" = \"ack\" ]; then printf \"terminal-ack\\n\"; elif [ \"$line\" = \"exit\" ]; then printf \"terminal-bye\\n\"; break; else printf \"echo:%s\\n\" \"$line\"; fi; done'\nwait-for: terminal-ready\ninput: ack\nwait-for: terminal-ack\ninput: exit\nwait-for: terminal-bye",
        expected_min_runtime_evidence: 6,
    },
    // --- Benchmark scenarios: real gym challenges ---
    BenchmarkScenario {
        id: "repo-exploration-deep-scan",
        title: "Deep repository structure analysis",
        description: "Explore the Simard repository itself to identify project structure, key source files, dependency graph from Cargo.toml, and executable entry points. Scored on completeness of structural discovery.",
        class: BenchmarkClass::RepoExploration,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Analyze the repository at /home/azureuser/src/Simard. Identify: (1) the top-level project structure and key directories, (2) all Cargo.toml dependencies and their purposes, (3) the main entry point(s) in src/main.rs or src/lib.rs, (4) at least five public modules and their responsibilities. Produce a structured summary covering all four areas.",
        expected_min_runtime_evidence: 4,
    },
    BenchmarkScenario {
        id: "doc-generation-public-fn",
        title: "Generate doc comment for public function",
        description: "Given a source file in the Simard repository, generate a syntactically valid Rust doc comment for a public function. Scored on whether the comment is valid rustdoc, mentions parameters and return type, and accurately describes behavior.",
        class: BenchmarkClass::Documentation,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Read the function `pub fn benchmark_scenarios()` in src/gym.rs. Generate a complete Rust doc comment (/// style) for it that: (1) describes what the function returns, (2) mentions the BenchmarkScenario type, (3) notes the static lifetime of the returned slice, (4) is syntactically valid rustdoc. Output the doc comment text.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "safe-code-change-add-derive",
        title: "Add Debug derive to a struct",
        description: "Execute a safe, minimal code change: add a #[derive(Debug)] attribute to a struct that lacks it. Scored on whether the change compiles successfully and the derive is correctly placed.",
        class: BenchmarkClass::SafeCodeChange,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Identify a public struct in the Simard codebase that does not derive Debug. Plan a minimal change to add #[derive(Debug)] to it. Describe the exact file, line, and change required. Verify the change would compile by checking for any generic parameter constraints that might prevent Debug derivation.",
        expected_min_runtime_evidence: 3,
    },
    BenchmarkScenario {
        id: "test-writing-unit-case",
        title: "Write a unit test for a public function",
        description: "Given a function signature, write a unit test that exercises its basic behavior. Scored on whether the test compiles, covers the primary code path, and uses appropriate assertions.",
        class: BenchmarkClass::TestWriting,
        identity: "simard-gym",
        base_type: "local-harness",
        topology: RuntimeTopology::SingleProcess,
        objective: "Write a unit test for the function `pub fn goal_slug(title: &str) -> String` defined in src/goals.rs. The test should: (1) call goal_slug with a representative input string containing uppercase letters, spaces, and special characters, (2) assert the output matches expected slug format (lowercase, hyphen-separated, no leading/trailing hyphens), (3) be a valid #[test] function that compiles and runs.",
        expected_min_runtime_evidence: 3,
    },
];

pub fn benchmark_scenarios() -> &'static [BenchmarkScenario] {
    &BENCHMARK_SCENARIOS
}

fn resolve_benchmark_scenario(scenario_id: &str) -> SimardResult<BenchmarkScenario> {
    benchmark_scenarios()
        .iter()
        .copied()
        .find(|candidate| candidate.id == scenario_id)
        .ok_or_else(|| SimardError::BenchmarkScenarioNotFound {
            scenario_id: scenario_id.to_string(),
        })
}

pub fn run_benchmark_scenario(
    scenario_id: &str,
    output_root: impl AsRef<Path>,
) -> SimardResult<BenchmarkRunReport> {
    let scenario = resolve_benchmark_scenario(scenario_id)?;
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

    for scenario in benchmark_scenarios().iter().copied() {
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

pub fn compare_latest_benchmark_runs(
    scenario_id: &str,
    output_root: impl AsRef<Path>,
) -> SimardResult<BenchmarkComparisonReport> {
    let scenario = resolve_benchmark_scenario(scenario_id)?;
    let output_root = output_root.as_ref();
    let mut reports = load_scenario_run_reports(scenario.id, output_root)?;
    if reports.len() < 2 {
        return Err(SimardError::BenchmarkComparisonUnavailable {
            scenario_id: scenario.id.to_string(),
            reason: format!(
                "need at least two completed runs under '{}'",
                display_path(&output_root.join(scenario.id))
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

    let current_summary = summarize_stored_run(&current);
    let previous_summary = summarize_stored_run(&previous);
    let delta = BenchmarkComparisonDelta {
        correctness_checks_passed: current_summary.correctness_checks_passed as i64
            - previous_summary.correctness_checks_passed as i64,
        unnecessary_action_count: benchmark_count_delta(
            current_summary.unnecessary_action_count,
            previous_summary.unnecessary_action_count,
        ),
        retry_count: benchmark_count_delta(
            current_summary.retry_count,
            previous_summary.retry_count,
        ),
        exported_memory_records: current_summary.exported_memory_records as i64
            - previous_summary.exported_memory_records as i64,
        exported_evidence_records: current_summary.exported_evidence_records as i64
            - previous_summary.exported_evidence_records as i64,
    };
    let status = compare_runs(&current_summary, &previous_summary);
    let summary = render_comparison_summary(status, &current_summary, &previous_summary, &delta);

    let comparison_dir = output_root
        .join("comparisons")
        .join(scenario.id)
        .join(format!(
            "{}-vs-{}",
            current_summary.session_id, previous_summary.session_id
        ));
    create_dir_all(&comparison_dir)?;
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
            report_json: display_path(&report_json),
            report_txt: display_path(&report_txt),
        },
    };
    write_json(&report_json, &report)?;
    write_text(&report_txt, render_text_comparison_report(&report))?;
    Ok(report)
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
    let restored = restore_from_handoff(&manifest, &request, &exported)?;
    metric_facts.record_required_action();
    let restored_snapshot = restored.snapshot()?;
    metric_facts.record_required_action();

    runtime.stop()?;
    metric_facts.record_required_action();
    let stopped_snapshot = runtime.snapshot()?;
    metric_facts.record_required_action();

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
    // Class-specific scoring checks.
    let class_checks = class_specific_checks(&scenario, &outcome, &exported);
    let checks = [checks, class_checks].concat();
    let passed = checks.iter().all(|check| check.passed);
    let run_dir = output_root
        .join(scenario.id)
        .join(outcome.session.id.as_str());
    create_dir_all(&run_dir)?;
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

/// Produce additional scenario-class-specific checks based on the scenario type.
///
/// These supplement the generic 8-check baseline with checks tailored to each
/// `BenchmarkClass`: structural discovery for repo exploration, doc validity
/// for documentation, compilation evidence for safe code changes, and test
/// structure for test writing scenarios.
fn class_specific_checks(
    scenario: &BenchmarkScenario,
    outcome: &crate::runtime::SessionOutcome,
    exported: &RuntimeHandoffSnapshot,
) -> Vec<BenchmarkCheckResult> {
    let summary = outcome.execution_summary.to_lowercase();
    let plan = outcome.plan.to_lowercase();
    let reflection = outcome.reflection.summary.to_lowercase();
    let combined = format!("{summary} {plan} {reflection}");

    match scenario.class {
        BenchmarkClass::RepoExploration => {
            let structure_mentioned = combined.contains("src/")
                || combined.contains("directory")
                || combined.contains("structure")
                || combined.contains("module");
            let deps_mentioned = combined.contains("cargo.toml")
                || combined.contains("dependenc")
                || combined.contains("crate");
            let entry_mentioned = combined.contains("main.rs")
                || combined.contains("lib.rs")
                || combined.contains("entry point")
                || combined.contains("entry-point");
            vec![
                BenchmarkCheckResult {
                    id: "repo-structure-discovered".to_string(),
                    passed: structure_mentioned,
                    detail: format!(
                        "execution output {} project structure references",
                        if structure_mentioned {
                            "contains"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "repo-dependencies-identified".to_string(),
                    passed: deps_mentioned,
                    detail: format!(
                        "execution output {} dependency references",
                        if deps_mentioned { "contains" } else { "lacks" }
                    ),
                },
                BenchmarkCheckResult {
                    id: "repo-entry-points-found".to_string(),
                    passed: entry_mentioned,
                    detail: format!(
                        "execution output {} entry point references",
                        if entry_mentioned { "contains" } else { "lacks" }
                    ),
                },
            ]
        }
        BenchmarkClass::Documentation => {
            let has_doc_syntax = combined.contains("///")
                || combined.contains("doc comment")
                || combined.contains("rustdoc")
                || combined.contains("documentation");
            let mentions_params = combined.contains("param")
                || combined.contains("argument")
                || combined.contains("return")
                || combined.contains("-> ");
            vec![
                BenchmarkCheckResult {
                    id: "doc-comment-syntax-valid".to_string(),
                    passed: has_doc_syntax,
                    detail: format!(
                        "execution output {} doc comment syntax",
                        if has_doc_syntax {
                            "references"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "doc-params-return-covered".to_string(),
                    passed: mentions_params,
                    detail: format!(
                        "execution output {} parameter/return documentation",
                        if mentions_params { "includes" } else { "lacks" }
                    ),
                },
            ]
        }
        BenchmarkClass::SafeCodeChange => {
            let compilation_evidence = combined.contains("compil")
                || combined.contains("cargo build")
                || combined.contains("cargo check")
                || combined.contains("build succeed")
                || combined.contains("no errors");
            let change_described = combined.contains("derive")
                || combined.contains("change")
                || combined.contains("modif")
                || combined.contains("diff");
            vec![
                BenchmarkCheckResult {
                    id: "code-change-compilation-checked".to_string(),
                    passed: compilation_evidence,
                    detail: format!(
                        "execution output {} compilation verification",
                        if compilation_evidence {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "code-change-described".to_string(),
                    passed: change_described,
                    detail: format!(
                        "execution output {} change description",
                        if change_described {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::TestWriting => {
            let has_test_annotation = combined.contains("#[test]")
                || combined.contains("test function")
                || combined.contains("unit test");
            let has_assertion = combined.contains("assert")
                || combined.contains("expect")
                || combined.contains("should_eq")
                || combined.contains("assert_eq");
            let covers_basic_case = combined.contains("input")
                || combined.contains("call")
                || combined.contains("invoke")
                || combined.contains("result");
            vec![
                BenchmarkCheckResult {
                    id: "test-structure-valid".to_string(),
                    passed: has_test_annotation,
                    detail: format!(
                        "execution output {} test annotation/structure",
                        if has_test_annotation {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "test-has-assertions".to_string(),
                    passed: has_assertion,
                    detail: format!(
                        "execution output {} assertions",
                        if has_assertion { "includes" } else { "lacks" }
                    ),
                },
                BenchmarkCheckResult {
                    id: "test-covers-basic-case".to_string(),
                    passed: covers_basic_case,
                    detail: format!(
                        "execution output {} basic case coverage",
                        if covers_basic_case {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::SessionQuality => {
            // Session quality scenarios rely on the generic checks.
            let session_summary_present =
                !outcome.execution_summary.trim().is_empty() && exported.memory_records.len() >= 2;
            vec![BenchmarkCheckResult {
                id: "session-quality-summary-adequate".to_string(),
                passed: session_summary_present,
                detail: format!(
                    "session produced {} memory records with {} execution summary",
                    exported.memory_records.len(),
                    if outcome.execution_summary.trim().is_empty() {
                        "empty"
                    } else {
                        "non-empty"
                    }
                ),
            }]
        }
    }
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
        format!(
            "Unnecessary actions: {}",
            render_benchmark_count(report.scorecard.unnecessary_action_count)
        ),
        format!(
            "Retry count: {}",
            render_benchmark_count(report.scorecard.retry_count)
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

fn render_text_comparison_report(report: &BenchmarkComparisonReport) -> String {
    [
        format!(
            "Scenario: {} ({})",
            report.scenario_id, report.scenario_title
        ),
        format!("Comparison status: {}", report.status),
        format!("Summary: {}", report.summary),
        format!("Current session: {}", report.current.session_id),
        format!("Current report: {}", report.current.report_json),
        format!(
            "Current unnecessary actions: {}",
            render_benchmark_count(report.current.unnecessary_action_count)
        ),
        format!(
            "Current retry count: {}",
            render_benchmark_count(report.current.retry_count)
        ),
        format!(
            "Current checks passed: {}/{}",
            report.current.correctness_checks_passed, report.current.correctness_checks_total
        ),
        format!("Previous session: {}", report.previous.session_id),
        format!("Previous report: {}", report.previous.report_json),
        format!(
            "Previous unnecessary actions: {}",
            render_benchmark_count(report.previous.unnecessary_action_count)
        ),
        format!(
            "Previous retry count: {}",
            render_benchmark_count(report.previous.retry_count)
        ),
        format!(
            "Previous checks passed: {}/{}",
            report.previous.correctness_checks_passed, report.previous.correctness_checks_total
        ),
        format!(
            "Delta correctness checks passed: {:+}",
            report.delta.correctness_checks_passed
        ),
        format!(
            "Delta unnecessary actions: {}",
            render_benchmark_delta(report.delta.unnecessary_action_count)
        ),
        format!(
            "Delta retry count: {}",
            render_benchmark_delta(report.delta.retry_count)
        ),
        format!(
            "Delta exported memory records: {:+}",
            report.delta.exported_memory_records
        ),
        format!(
            "Delta exported evidence records: {:+}",
            report.delta.exported_evidence_records
        ),
    ]
    .join("\n")
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

fn load_scenario_run_reports(
    scenario_id: &str,
    output_root: &Path,
) -> SimardResult<Vec<StoredBenchmarkRunArtifact>> {
    let scenario_dir = output_root.join(scenario_id);
    let entries = match fs::read_dir(&scenario_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => {
            return Err(SimardError::ArtifactIo {
                path: scenario_dir,
                reason: error.to_string(),
            });
        }
    };
    let mut reports = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| SimardError::ArtifactIo {
            path: scenario_dir.clone(),
            reason: error.to_string(),
        })?;
        let report_path = entry.path().join("report.json");
        if !report_path.is_file() {
            continue;
        }
        let report = load_stored_run_report(&report_path)?;
        if report.scenario.id == scenario_id {
            reports.push(StoredBenchmarkRunArtifact {
                report_path,
                report,
            });
        }
    }
    Ok(reports)
}

fn load_stored_run_report(path: &Path) -> SimardResult<StoredBenchmarkRunReport> {
    let raw = fs::read_to_string(path).map_err(|error| SimardError::ArtifactIo {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    serde_json::from_str(&raw).map_err(|error| SimardError::ArtifactIo {
        path: path.to_path_buf(),
        reason: format!("invalid benchmark report JSON: {error}"),
    })
}

fn summarize_stored_run(run: &StoredBenchmarkRunArtifact) -> BenchmarkComparisonRunSummary {
    BenchmarkComparisonRunSummary {
        suite_id: run.report.suite_id.clone(),
        session_id: run.report.session_id.clone(),
        run_started_at_unix_ms: run.report.run_started_at_unix_ms,
        passed: run.report.passed,
        correctness_checks_passed: run.report.scorecard.correctness_checks_passed,
        correctness_checks_total: run.report.scorecard.correctness_checks_total,
        evidence_quality: run.report.scorecard.evidence_quality.clone(),
        unnecessary_action_count: run.report.scorecard.unnecessary_action_count,
        retry_count: run.report.scorecard.retry_count,
        exported_memory_records: run.report.handoff.exported_memory_records,
        exported_evidence_records: run.report.handoff.exported_evidence_records,
        report_json: display_path(&run.report_path),
    }
}

fn compare_runs(
    current: &BenchmarkComparisonRunSummary,
    previous: &BenchmarkComparisonRunSummary,
) -> BenchmarkComparisonStatus {
    if current.passed != previous.passed {
        return if current.passed {
            BenchmarkComparisonStatus::Improved
        } else {
            BenchmarkComparisonStatus::Regressed
        };
    }
    if current.correctness_checks_passed != previous.correctness_checks_passed {
        return if current.correctness_checks_passed > previous.correctness_checks_passed {
            BenchmarkComparisonStatus::Improved
        } else {
            BenchmarkComparisonStatus::Regressed
        };
    }
    if current.exported_evidence_records != previous.exported_evidence_records {
        return if current.exported_evidence_records > previous.exported_evidence_records {
            BenchmarkComparisonStatus::Improved
        } else {
            BenchmarkComparisonStatus::Regressed
        };
    }
    if current.exported_memory_records != previous.exported_memory_records {
        return if current.exported_memory_records > previous.exported_memory_records {
            BenchmarkComparisonStatus::Improved
        } else {
            BenchmarkComparisonStatus::Regressed
        };
    }
    if let Some(status) = compare_lower_is_better(
        current.unnecessary_action_count,
        previous.unnecessary_action_count,
    ) {
        return status;
    }
    if let Some(status) = compare_lower_is_better(current.retry_count, previous.retry_count) {
        return status;
    }
    match evidence_quality_rank(&current.evidence_quality)
        .cmp(&evidence_quality_rank(&previous.evidence_quality))
    {
        std::cmp::Ordering::Greater => BenchmarkComparisonStatus::Improved,
        std::cmp::Ordering::Less => BenchmarkComparisonStatus::Regressed,
        std::cmp::Ordering::Equal => BenchmarkComparisonStatus::Unchanged,
    }
}

fn evidence_quality_rank(value: &str) -> u8 {
    match value {
        "sufficient" => 2,
        "thin" => 1,
        _ => 0,
    }
}

fn render_comparison_summary(
    status: BenchmarkComparisonStatus,
    current: &BenchmarkComparisonRunSummary,
    previous: &BenchmarkComparisonRunSummary,
    delta: &BenchmarkComparisonDelta,
) -> String {
    let unnecessary_action_delta = render_benchmark_delta(delta.unnecessary_action_count);
    let retry_delta = render_benchmark_delta(delta.retry_count);
    match status {
        BenchmarkComparisonStatus::Improved => format!(
            "latest run improved from session '{}' to '{}' with check delta {:+}, unnecessary-action delta {}, retry delta {}, memory delta {:+}, and evidence delta {:+}",
            previous.session_id,
            current.session_id,
            delta.correctness_checks_passed,
            unnecessary_action_delta,
            retry_delta,
            delta.exported_memory_records,
            delta.exported_evidence_records
        ),
        BenchmarkComparisonStatus::Regressed => format!(
            "latest run regressed from session '{}' to '{}' with check delta {:+}, unnecessary-action delta {}, retry delta {}, memory delta {:+}, and evidence delta {:+}",
            previous.session_id,
            current.session_id,
            delta.correctness_checks_passed,
            unnecessary_action_delta,
            retry_delta,
            delta.exported_memory_records,
            delta.exported_evidence_records
        ),
        BenchmarkComparisonStatus::Unchanged => format!(
            "latest run matched session '{}' on pass/fail status and checks, with unnecessary-action delta {}, retry delta {}, memory delta {:+}, and evidence delta {:+}",
            previous.session_id,
            unnecessary_action_delta,
            retry_delta,
            delta.exported_memory_records,
            delta.exported_evidence_records
        ),
    }
}

fn derive_benchmark_metrics(facts: &BenchmarkMetricFacts) -> DerivedBenchmarkMetrics {
    DerivedBenchmarkMetrics {
        unnecessary_action_count: facts.actions.iter().try_fold(0_u32, |count, fact| {
            match fact.classification {
                Some(BenchmarkActionClassification::Required) => Some(count),
                Some(BenchmarkActionClassification::Unnecessary) => count.checked_add(1),
                None => None,
            }
        }),
        retry_count: facts.attempts.iter().try_fold(0_u32, |count, fact| {
            match fact.classification {
                Some(BenchmarkAttemptClassification::Primary) => Some(count),
                Some(BenchmarkAttemptClassification::Retry) => count.checked_add(1),
                None => None,
            }
        }),
    }
}

fn benchmark_count_delta(current: Option<u32>, previous: Option<u32>) -> Option<i64> {
    match (current, previous) {
        (Some(current), Some(previous)) => Some(current as i64 - previous as i64),
        _ => None,
    }
}

fn compare_lower_is_better(
    current: Option<u32>,
    previous: Option<u32>,
) -> Option<BenchmarkComparisonStatus> {
    match (current, previous) {
        (Some(current), Some(previous)) if current != previous => Some(if current < previous {
            BenchmarkComparisonStatus::Improved
        } else {
            BenchmarkComparisonStatus::Regressed
        }),
        _ => None,
    }
}

pub(crate) fn render_benchmark_count(value: Option<u32>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "unmeasured".to_string(),
    }
}

pub(crate) fn render_benchmark_delta(value: Option<i64>) -> String {
    match value {
        Some(value) => format!("{value:+}"),
        None => "unmeasured".to_string(),
    }
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

#[cfg(test)]
mod tests {
    use super::{BenchmarkMetricFacts, derive_benchmark_metrics};

    #[test]
    fn metric_derivation_counts_retries_and_unnecessary_actions_from_recorded_facts() {
        let mut facts = BenchmarkMetricFacts::default();
        facts.record_primary_attempt();
        facts.record_retry_attempt();
        facts.record_required_action();
        facts.record_unnecessary_action();
        facts.record_unnecessary_action();

        let derived = derive_benchmark_metrics(&facts);

        assert_eq!(derived.retry_count, Some(1));
        assert_eq!(derived.unnecessary_action_count, Some(2));
    }

    #[test]
    fn metric_derivation_returns_unmeasured_when_facts_are_incomplete() {
        let mut facts = BenchmarkMetricFacts::default();
        facts.record_primary_attempt();
        facts.record_unmeasured_attempt();
        facts.record_required_action();
        facts.record_unmeasured_action();

        let derived = derive_benchmark_metrics(&facts);

        assert_eq!(derived.retry_count, None);
        assert_eq!(derived.unnecessary_action_count, None);
    }
}
