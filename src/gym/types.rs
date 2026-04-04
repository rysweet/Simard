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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_class_display_all_variants() {
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
    }

    #[test]
    fn comparison_status_display_all_variants() {
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
    fn benchmark_class_serializes_to_kebab_case() {
        let json = serde_json::to_string(&BenchmarkClass::RepoExploration).unwrap();
        assert!(json.contains("repo-exploration"));
        let json = serde_json::to_string(&BenchmarkClass::SafeCodeChange).unwrap();
        assert!(json.contains("safe-code-change"));
        let json = serde_json::to_string(&BenchmarkClass::TestWriting).unwrap();
        assert!(json.contains("test-writing"));
    }

    #[test]
    fn comparison_status_serializes_to_kebab_case() {
        let json = serde_json::to_string(&BenchmarkComparisonStatus::Improved).unwrap();
        assert!(json.contains("improved"));
        let json = serde_json::to_string(&BenchmarkComparisonStatus::Unchanged).unwrap();
        assert!(json.contains("unchanged"));
        let json = serde_json::to_string(&BenchmarkComparisonStatus::Regressed).unwrap();
        assert!(json.contains("regressed"));
    }

    #[test]
    fn benchmark_class_equality_and_inequality() {
        assert_eq!(
            BenchmarkClass::RepoExploration,
            BenchmarkClass::RepoExploration
        );
        assert_ne!(
            BenchmarkClass::RepoExploration,
            BenchmarkClass::Documentation
        );
        assert_ne!(BenchmarkClass::SafeCodeChange, BenchmarkClass::TestWriting);
    }

    #[test]
    fn comparison_status_equality_and_inequality() {
        assert_eq!(
            BenchmarkComparisonStatus::Improved,
            BenchmarkComparisonStatus::Improved
        );
        assert_ne!(
            BenchmarkComparisonStatus::Improved,
            BenchmarkComparisonStatus::Regressed
        );
        assert_ne!(
            BenchmarkComparisonStatus::Unchanged,
            BenchmarkComparisonStatus::Regressed
        );
    }

    #[test]
    fn benchmark_class_copy_semantics() {
        let original = BenchmarkClass::Documentation;
        let copied = original;
        assert_eq!(original, copied);
    }

    #[test]
    fn comparison_status_copy_semantics() {
        let original = BenchmarkComparisonStatus::Regressed;
        let copied = original;
        assert_eq!(original, copied);
    }

    #[test]
    fn benchmark_check_result_construction_passed() {
        let result = BenchmarkCheckResult {
            id: "test-check".to_string(),
            passed: true,
            detail: "all good".to_string(),
        };
        assert_eq!(result.id, "test-check");
        assert!(result.passed);
        assert_eq!(result.detail, "all good");
    }

    #[test]
    fn benchmark_check_result_construction_failed() {
        let result = BenchmarkCheckResult {
            id: "fail-check".to_string(),
            passed: false,
            detail: "something broke".to_string(),
        };
        assert!(!result.passed);
    }

    #[test]
    fn benchmark_artifact_paths_all_fields() {
        let paths = BenchmarkArtifactPaths {
            run_dir: "/output/run1".to_string(),
            report_json: "/output/run1/report.json".to_string(),
            report_txt: "/output/run1/report.txt".to_string(),
            review_json: "/output/run1/review.json".to_string(),
        };
        assert_eq!(paths.run_dir, "/output/run1");
        assert_eq!(paths.report_json, "/output/run1/report.json");
        assert_eq!(paths.report_txt, "/output/run1/report.txt");
        assert_eq!(paths.review_json, "/output/run1/review.json");
    }

    #[test]
    fn benchmark_runtime_report_fields() {
        let r = BenchmarkRuntimeReport {
            identity: "test-id".to_string(),
            selected_base_type: "local-harness".to_string(),
            topology: "single-process".to_string(),
            adapter_implementation: "test-adapter".to_string(),
            topology_backend: "loopback".to_string(),
            transport_backend: "loopback".to_string(),
            supervisor_backend: "coordinated".to_string(),
            runtime_node: "node-1".to_string(),
            mailbox_address: "addr-1".to_string(),
            snapshot_state_before_stop: "ready".to_string(),
            snapshot_state_after_stop: "stopped".to_string(),
        };
        assert_eq!(r.identity, "test-id");
        assert_eq!(r.snapshot_state_before_stop, "ready");
        assert_eq!(r.snapshot_state_after_stop, "stopped");
    }

    #[test]
    fn benchmark_handoff_report_fields() {
        let h = BenchmarkHandoffReport {
            exported_state: "stopped".to_string(),
            exported_memory_records: 5,
            exported_evidence_records: 4,
            restored_runtime_state: "ready".to_string(),
            restored_session_phase: Some("complete".to_string()),
            restored_session_objective: None,
        };
        assert_eq!(h.exported_memory_records, 5);
        assert_eq!(h.exported_evidence_records, 4);
        assert!(h.restored_session_objective.is_none());
        assert_eq!(h.restored_session_phase.unwrap(), "complete");
    }

    #[test]
    fn benchmark_scorecard_fields() {
        let s = BenchmarkScorecard {
            task_completed: true,
            evidence_quality: "sufficient".to_string(),
            correctness_checks_passed: 7,
            correctness_checks_total: 10,
            unnecessary_action_count: Some(2),
            retry_count: None,
            human_review_notes: vec!["note1".to_string()],
            measurement_notes: vec!["m1".to_string(), "m2".to_string()],
        };
        assert!(s.task_completed);
        assert_eq!(s.correctness_checks_passed, 7);
        assert_eq!(s.unnecessary_action_count, Some(2));
        assert_eq!(s.retry_count, None);
        assert_eq!(s.human_review_notes.len(), 1);
        assert_eq!(s.measurement_notes.len(), 2);
    }

    #[test]
    fn benchmark_comparison_delta_fields() {
        let d = BenchmarkComparisonDelta {
            correctness_checks_passed: 2,
            unnecessary_action_count: Some(-1),
            retry_count: None,
            exported_memory_records: 3,
            exported_evidence_records: -1,
        };
        assert_eq!(d.correctness_checks_passed, 2);
        assert_eq!(d.unnecessary_action_count, Some(-1));
        assert_eq!(d.retry_count, None);
        assert_eq!(d.exported_memory_records, 3);
        assert_eq!(d.exported_evidence_records, -1);
    }

    #[test]
    fn benchmark_suite_scenario_summary_fields() {
        let s = BenchmarkSuiteScenarioSummary {
            scenario_id: "test-scenario".to_string(),
            passed: false,
            session_id: "session-abc".to_string(),
            report_json: "/path/report.json".to_string(),
        };
        assert_eq!(s.scenario_id, "test-scenario");
        assert!(!s.passed);
        assert_eq!(s.session_id, "session-abc");
    }

    #[test]
    fn benchmark_suite_report_fields() {
        let r = BenchmarkSuiteReport {
            suite_id: "starter".to_string(),
            run_started_at_unix_ms: 999,
            passed: true,
            scenarios: vec![],
            artifact_path: "/path/suite.json".to_string(),
        };
        assert!(r.passed);
        assert!(r.scenarios.is_empty());
        assert_eq!(r.suite_id, "starter");
    }

    #[test]
    fn benchmark_comparison_artifact_paths_fields() {
        let p = BenchmarkComparisonArtifactPaths {
            report_json: "/cmp/report.json".to_string(),
            report_txt: "/cmp/report.txt".to_string(),
        };
        assert_eq!(p.report_json, "/cmp/report.json");
        assert_eq!(p.report_txt, "/cmp/report.txt");
    }

    #[test]
    fn benchmark_scenario_construction_and_copy() {
        let scenario = BenchmarkScenario {
            id: "test-id",
            title: "Test Title",
            description: "Test description",
            class: BenchmarkClass::RepoExploration,
            identity: "test-identity",
            base_type: "local-harness",
            topology: RuntimeTopology::SingleProcess,
            objective: "Test objective",
            expected_min_runtime_evidence: 3,
        };
        let copied = scenario;
        assert_eq!(scenario.id, copied.id);
        assert_eq!(scenario.class, BenchmarkClass::RepoExploration);
        assert_eq!(copied.expected_min_runtime_evidence, 3);
    }

    #[test]
    fn benchmark_check_result_serializes_to_json() {
        let result = BenchmarkCheckResult {
            id: "chk".to_string(),
            passed: true,
            detail: "ok".to_string(),
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["id"], "chk");
        assert_eq!(json["passed"], true);
        assert_eq!(json["detail"], "ok");
    }

    #[test]
    fn benchmark_comparison_run_summary_fields() {
        let s = BenchmarkComparisonRunSummary {
            suite_id: "starter".to_string(),
            session_id: "session-abc".to_string(),
            run_started_at_unix_ms: 12345,
            passed: true,
            correctness_checks_passed: 5,
            correctness_checks_total: 8,
            evidence_quality: "sufficient".to_string(),
            unnecessary_action_count: Some(0),
            retry_count: Some(1),
            exported_memory_records: 10,
            exported_evidence_records: 8,
            report_json: "/path/report.json".to_string(),
        };
        assert!(s.passed);
        assert_eq!(s.correctness_checks_passed, 5);
        assert_eq!(s.exported_memory_records, 10);
    }
}
