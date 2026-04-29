//! Class-specific check builders for `BenchmarkScenario`s.

use super::super::types::{BenchmarkCheckResult, BenchmarkClass, BenchmarkScenario};
use crate::handoff::RuntimeHandoffSnapshot;

pub(crate) fn class_specific_checks(
    scenario: &BenchmarkScenario,
    outcome: &crate::runtime::SessionOutcome,
    exported: &RuntimeHandoffSnapshot,
) -> Vec<BenchmarkCheckResult> {
    let summary = outcome.execution_summary.to_lowercase();
    let plan = outcome.plan.to_lowercase();
    let reflection = outcome.reflection.summary.to_lowercase();
    let combined = format!("{summary} {plan} {reflection}");
    let _ = exported;
    let _ = outcome;

    match scenario.class {
        BenchmarkClass::RepoExploration => super::checks_5::checks_for_repo_exploration(&combined),
        BenchmarkClass::Documentation => super::checks_3::checks_for_documentation(&combined),
        BenchmarkClass::SafeCodeChange => super::checks_3::checks_for_safe_code_change(&combined),
        BenchmarkClass::TestWriting => super::checks_6::checks_for_test_writing(&combined),
        BenchmarkClass::SessionQuality => {
            super::checks_5::checks_for_session_quality(&combined, outcome, exported)
        }
        BenchmarkClass::BugFix => super::checks_1::checks_for_bug_fix(&combined),
        BenchmarkClass::Refactoring => super::checks_4::checks_for_refactoring(&combined),
        BenchmarkClass::DependencyAnalysis => {
            super::checks_1::checks_for_dependency_analysis(&combined)
        }
        BenchmarkClass::ErrorHandling => super::checks_4::checks_for_error_handling(&combined),
        BenchmarkClass::PerformanceAnalysis => {
            super::checks_3::checks_for_performance_analysis(&combined)
        }
        BenchmarkClass::SecurityAudit => super::checks_3::checks_for_security_audit(&combined),
        BenchmarkClass::ApiDesign => super::checks_4::checks_for_api_design(&combined),
        BenchmarkClass::CodeReview => super::checks_2::checks_for_code_review(&combined),
        BenchmarkClass::Debugging => super::checks_6::checks_for_debugging(&combined),
        BenchmarkClass::ConfigManagement => {
            super::checks_6::checks_for_config_management(&combined)
        }
        BenchmarkClass::ConcurrencyAnalysis => {
            super::checks_2::checks_for_concurrency_analysis(&combined)
        }
        BenchmarkClass::MigrationPlanning => {
            super::checks_2::checks_for_migration_planning(&combined)
        }
        BenchmarkClass::ObservabilityInstrumentation => {
            super::checks_5::checks_for_observability_instrumentation(&combined)
        }
        BenchmarkClass::DataModeling => super::checks_5::checks_for_data_modeling(&combined),
        BenchmarkClass::DataMigration => super::checks_1::checks_for_data_migration(&combined),
        BenchmarkClass::CicdPipeline => super::checks_2::checks_for_cicd_pipeline(&combined),
        BenchmarkClass::DependencyUpgrade => {
            super::checks_1::checks_for_dependency_upgrade(&combined)
        }
        BenchmarkClass::ReleaseManagement => {
            super::checks_3::checks_for_release_management(&combined)
        }
        BenchmarkClass::AccessibilityReview => {
            super::checks_2::checks_for_accessibility_review(&combined)
        }
        BenchmarkClass::InternationalizationReview => {
            super::checks_1::checks_for_internationalization_review(&combined)
        }
        BenchmarkClass::IncidentResponse => {
            super::checks_3::checks_for_incident_response(&combined)
        }
        BenchmarkClass::DatabaseSchemaChange => {
            super::checks_4::checks_for_database_schema_change(&combined)
        }
        BenchmarkClass::CachingStrategy => super::checks_5::checks_for_caching_strategy(&combined),
        BenchmarkClass::FeatureFlagging => super::checks_6::checks_for_feature_flagging(&combined),
        BenchmarkClass::RateLimiting => super::checks_4::checks_for_rate_limiting(&combined),
        BenchmarkClass::EventSourcing => super::checks_6::checks_for_event_sourcing(&combined),
        BenchmarkClass::ChaosEngineering => {
            super::checks_5::checks_for_chaos_engineering(&combined)
        }
        BenchmarkClass::KnowledgeRecall => match scenario.id {
            "knowledge-recall-repo-ooda-loop-layout"
            | "knowledge-recall-repo-cognitive-memory-store"
            | "knowledge-recall-repo-engineer-worktree-pattern" => {
                super::checks_7::checks_for_knowledge_recall_repo(scenario, &combined, exported)
            }
            _ => super::checks_6::checks_for_knowledge_recall(scenario, &combined, exported),
        },
    }
}
