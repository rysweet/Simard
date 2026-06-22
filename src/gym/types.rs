use std::fmt::{self, Display, Formatter};

use serde::Serialize;

use crate::gym_scoring::EvidenceQualityAssessment;
use crate::runtime::RuntimeTopology;

/// The V1 benchmark classes sanctioned by `Specs/ProductArchitecture.md`
/// ("Initial Benchmark Classes", lines 196-201).
///
/// The spec is explicit (line 214): "V1 should prefer a small benchmark set
/// with high signal over a large, noisy suite." The gym therefore exposes only
/// these four high-signal classes. Additional speculative classes were removed
/// (issue #2087) to keep the suite focused on changing engineering decisions
/// rather than creating dashboard theater.
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

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BenchmarkScorecard {
    pub task_completed: bool,
    /// Coarse legacy categorical (`sufficient`/`thin`) retained for the
    /// reporting and run-comparison paths. Prefer [`Self::evidence_quality_assessment`]
    /// for meaningful, comparable evidence-quality signal.
    pub evidence_quality: String,
    /// Structured 5-dimension evidence-quality assessment. This is the key
    /// scoring field per `Specs/ProductArchitecture.md` (lines 204-213): it
    /// distinguishes quality differences between runs that the binary
    /// categorical cannot.
    pub evidence_quality_assessment: EvidenceQualityAssessment,
    pub correctness_checks_passed: usize,
    pub correctness_checks_total: usize,
    pub unnecessary_action_count: Option<u32>,
    pub retry_count: Option<u32>,
    pub human_review_notes: Vec<String>,
    pub measurement_notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
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
    /// `true` when the scenario was skipped (e.g. auth unavailable).
    /// Skipped scenarios do not count as failures for suite-level pass/fail.
    #[serde(default, skip_serializing_if = "is_false")]
    pub skipped: bool,
    /// Human-readable reason the scenario was skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    pub session_id: String,
    pub report_json: String,
}

fn is_false(v: &bool) -> bool {
    !v
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
