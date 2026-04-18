use std::fmt::{self, Display, Formatter};

use serde::Serialize;

use crate::runtime::RuntimeTopology;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BenchmarkClass {
    RepoExploration,
    Documentation,
    SafeCodeChange,
    SessionQuality,
    TestWriting,
    BugFix,
    Refactoring,
    DependencyAnalysis,
    ErrorHandling,
    PerformanceAnalysis,
    SecurityAudit,
    ApiDesign,
    CodeReview,
    Debugging,
    ConfigManagement,
    ConcurrencyAnalysis,
    MigrationPlanning,
    ObservabilityInstrumentation,
    DataModeling,
    DataMigration,
    CicdPipeline,
    DependencyUpgrade,
    ReleaseManagement,
}

impl Display for BenchmarkClass {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::RepoExploration => "repo-exploration",
            Self::Documentation => "documentation",
            Self::SafeCodeChange => "safe-code-change",
            Self::SessionQuality => "session-quality",
            Self::TestWriting => "test-writing",
            Self::BugFix => "bug-fix",
            Self::Refactoring => "refactoring",
            Self::DependencyAnalysis => "dependency-analysis",
            Self::ErrorHandling => "error-handling",
            Self::PerformanceAnalysis => "performance-analysis",
            Self::SecurityAudit => "security-audit",
            Self::ApiDesign => "api-design",
            Self::CodeReview => "code-review",
            Self::Debugging => "debugging",
            Self::ConfigManagement => "config-management",
            Self::ConcurrencyAnalysis => "concurrency-analysis",
            Self::MigrationPlanning => "migration-planning",
            Self::ObservabilityInstrumentation => "observability-instrumentation",
            Self::DataModeling => "data-modeling",
            Self::DataMigration => "data-migration",
            Self::CicdPipeline => "cicd-pipeline",
            Self::DependencyUpgrade => "dependency-upgrade",
            Self::ReleaseManagement => "release-management",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_benchmark_class_display() {
        assert_eq!(
            BenchmarkClass::RepoExploration.to_string(),
            "repo-exploration"
        );
        assert_eq!(BenchmarkClass::Documentation.to_string(), "documentation");
        assert_eq!(
            BenchmarkClass::SafeCodeChange.to_string(),
            "safe-code-change"
        );
        assert_eq!(
            BenchmarkClass::SessionQuality.to_string(),
            "session-quality"
        );
        assert_eq!(BenchmarkClass::TestWriting.to_string(), "test-writing");
        assert_eq!(BenchmarkClass::BugFix.to_string(), "bug-fix");
        assert_eq!(BenchmarkClass::Refactoring.to_string(), "refactoring");
        assert_eq!(
            BenchmarkClass::DependencyAnalysis.to_string(),
            "dependency-analysis"
        );
        assert_eq!(BenchmarkClass::ErrorHandling.to_string(), "error-handling");
        assert_eq!(
            BenchmarkClass::PerformanceAnalysis.to_string(),
            "performance-analysis"
        );
        assert_eq!(BenchmarkClass::SecurityAudit.to_string(), "security-audit");
        assert_eq!(BenchmarkClass::ApiDesign.to_string(), "api-design");
        assert_eq!(BenchmarkClass::CodeReview.to_string(), "code-review");
        assert_eq!(BenchmarkClass::Debugging.to_string(), "debugging");
        assert_eq!(
            BenchmarkClass::ConfigManagement.to_string(),
            "config-management"
        );
        assert_eq!(
            BenchmarkClass::ConcurrencyAnalysis.to_string(),
            "concurrency-analysis"
        );
        assert_eq!(
            BenchmarkClass::MigrationPlanning.to_string(),
            "migration-planning"
        );
        assert_eq!(
            BenchmarkClass::ObservabilityInstrumentation.to_string(),
            "observability-instrumentation"
        );
        assert_eq!(BenchmarkClass::DataModeling.to_string(), "data-modeling");
        assert_eq!(BenchmarkClass::DataMigration.to_string(), "data-migration");
        assert_eq!(BenchmarkClass::CicdPipeline.to_string(), "cicd-pipeline");
        assert_eq!(
            BenchmarkClass::DependencyUpgrade.to_string(),
            "dependency-upgrade"
        );
        assert_eq!(
            BenchmarkClass::ReleaseManagement.to_string(),
            "release-management"
        );
    }

    #[test]
    fn test_benchmark_comparison_status_display() {
        assert_eq!(BenchmarkComparisonStatus::Improved.to_string(), "improved");
        assert_eq!(
            BenchmarkComparisonStatus::Unchanged.to_string(),
            "unchanged"
        );
        assert_eq!(
            BenchmarkComparisonStatus::Regressed.to_string(),
            "regressed"
        );
    }

    #[test]
    fn test_benchmark_class_serialize_kebab_case() {
        let json = serde_json::to_string(&BenchmarkClass::RepoExploration).unwrap();
        assert_eq!(json, r##""repo-exploration""##);
        let json = serde_json::to_string(&BenchmarkClass::SafeCodeChange).unwrap();
        assert_eq!(json, r##""safe-code-change""##);
    }

    #[test]
    fn test_benchmark_comparison_status_serialize_kebab_case() {
        let json = serde_json::to_string(&BenchmarkComparisonStatus::Improved).unwrap();
        assert_eq!(json, r##""improved""##);
        let json = serde_json::to_string(&BenchmarkComparisonStatus::Regressed).unwrap();
        assert_eq!(json, r##""regressed""##);
    }

    #[test]
    fn test_benchmark_check_result_serialize() {
        let result = BenchmarkCheckResult {
            id: "check-1".to_string(),
            passed: true,
            detail: "all good".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains(r##""passed":true"##));
        assert!(json.contains(r##""id":"check-1""##));
    }

    #[test]
    fn test_benchmark_artifact_paths_serialize() {
        let paths = BenchmarkArtifactPaths {
            run_dir: "/runs/1".to_string(),
            report_json: "/runs/1/report.json".to_string(),
            report_txt: "/runs/1/report.txt".to_string(),
            review_json: "/runs/1/review.json".to_string(),
        };
        let json = serde_json::to_string(&paths).unwrap();
        assert!(json.contains("report.json"));
        assert!(json.contains("review.json"));
    }

    #[test]
    fn test_benchmark_scorecard_serialize() {
        let scorecard = BenchmarkScorecard {
            task_completed: true,
            evidence_quality: "high".to_string(),
            correctness_checks_passed: 8,
            correctness_checks_total: 10,
            unnecessary_action_count: Some(2),
            retry_count: None,
            human_review_notes: vec!["looks good".to_string()],
            measurement_notes: vec![],
        };
        let json = serde_json::to_string(&scorecard).unwrap();
        assert!(json.contains(r##""task_completed":true"##));
        assert!(json.contains(r##""correctness_checks_passed":8"##));
    }

    #[test]
    fn test_benchmark_comparison_delta_serialize() {
        let delta = BenchmarkComparisonDelta {
            correctness_checks_passed: 2,
            unnecessary_action_count: Some(-1),
            retry_count: None,
            exported_memory_records: 5,
            exported_evidence_records: -3,
        };
        let json = serde_json::to_string(&delta).unwrap();
        assert!(json.contains(r##""correctness_checks_passed":2"##));
        assert!(json.contains(r##""exported_evidence_records":-3"##));
    }

    #[test]
    fn test_benchmark_class_eq() {
        assert_eq!(BenchmarkClass::BugFix, BenchmarkClass::BugFix);
        assert_ne!(BenchmarkClass::BugFix, BenchmarkClass::Refactoring);
    }

    #[test]
    fn test_benchmark_class_copy() {
        let a = BenchmarkClass::TestWriting;
        let b = a; // copy
        assert_eq!(a, b);
    }

    #[test]
    fn test_benchmark_suite_report_serialize() {
        let report = BenchmarkSuiteReport {
            suite_id: "suite-1".to_string(),
            run_started_at_unix_ms: 1700000000000,
            passed: true,
            scenarios: vec![],
            artifact_path: "/artifacts".to_string(),
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains(r##""passed":true"##));
        assert!(json.contains(r##""scenarios":[]"##));
    }

    #[test]
    fn test_benchmark_handoff_report_serialize() {
        let report = BenchmarkHandoffReport {
            exported_state: "persisting".to_string(),
            exported_memory_records: 5,
            exported_evidence_records: 3,
            restored_runtime_state: "ready".to_string(),
            restored_session_phase: Some("planning".to_string()),
            restored_session_objective: None,
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains(r##""exported_memory_records":5"##));
        assert!(json.contains(r##""restored_session_objective":null"##));
    }

    #[test]
    fn test_benchmark_runtime_report_serialize() {
        let report = BenchmarkRuntimeReport {
            identity: "test".to_string(),
            selected_base_type: "local".to_string(),
            topology: "single-process".to_string(),
            adapter_implementation: "mock".to_string(),
            topology_backend: "inmemory".to_string(),
            transport_backend: "inmemory".to_string(),
            supervisor_backend: "inmemory".to_string(),
            runtime_node: "node-local".to_string(),
            mailbox_address: "inmemory://node-local".to_string(),
            snapshot_state_before_stop: "active".to_string(),
            snapshot_state_after_stop: "persisting".to_string(),
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains(r##""identity":"test""##));
    }
}
