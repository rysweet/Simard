use super::types::*;

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
    assert_eq!(
        BenchmarkClass::AccessibilityReview.to_string(),
        "accessibility-review"
    );
    assert_eq!(
        BenchmarkClass::InternationalizationReview.to_string(),
        "internationalization-review"
    );
    assert_eq!(
        BenchmarkClass::IncidentResponse.to_string(),
        "incident-response"
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
