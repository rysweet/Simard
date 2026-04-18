use super::types::*;
use crate::runtime::RuntimeTopology;
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
    let json = serde_json::to_string(&BenchmarkClass::BugFix).unwrap();
    assert!(json.contains("bug-fix"));
    let json = serde_json::to_string(&BenchmarkClass::Refactoring).unwrap();
    assert!(json.contains("refactoring"));
    let json = serde_json::to_string(&BenchmarkClass::DependencyAnalysis).unwrap();
    assert!(json.contains("dependency-analysis"));
    let json = serde_json::to_string(&BenchmarkClass::ErrorHandling).unwrap();
    assert!(json.contains("error-handling"));
    let json = serde_json::to_string(&BenchmarkClass::PerformanceAnalysis).unwrap();
    assert!(json.contains("performance-analysis"));
    let json = serde_json::to_string(&BenchmarkClass::SecurityAudit).unwrap();
    assert!(json.contains("security-audit"));
    let json = serde_json::to_string(&BenchmarkClass::ApiDesign).unwrap();
    assert!(json.contains("api-design"));
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
