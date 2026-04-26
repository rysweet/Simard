#[cfg(test)]
mod tests {
    use crate::gym::scenarios::*;
    use crate::gym::types::BenchmarkClass;

    #[test]
    fn benchmark_scenarios_not_empty() {
        let scenarios = benchmark_scenarios();
        assert!(!scenarios.is_empty());
    }

    #[test]
    fn benchmark_scenarios_ids_are_unique() {
        let scenarios = benchmark_scenarios();
        let ids: Vec<_> = scenarios.iter().map(|s| s.id).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), unique.len(), "scenario IDs must be unique");
    }

    #[test]
    fn resolve_known_scenario() {
        let scenario = resolve_benchmark_scenario("repo-exploration-local").unwrap();
        assert_eq!(scenario.id, "repo-exploration-local");
        assert_eq!(scenario.class, BenchmarkClass::RepoExploration);
    }

    #[test]
    fn resolve_unknown_scenario_errors() {
        let result = resolve_benchmark_scenario("nonexistent-scenario");
        assert!(result.is_err());
    }

    #[test]
    fn all_scenarios_have_nonempty_fields() {
        for scenario in benchmark_scenarios() {
            assert!(!scenario.id.is_empty(), "id must be non-empty");
            assert!(
                !scenario.title.is_empty(),
                "title must be non-empty for {}",
                scenario.id
            );
            assert!(
                !scenario.description.is_empty(),
                "description must be non-empty for {}",
                scenario.id
            );
            assert!(
                !scenario.identity.is_empty(),
                "identity must be non-empty for {}",
                scenario.id
            );
            assert!(
                !scenario.base_type.is_empty(),
                "base_type must be non-empty for {}",
                scenario.id
            );
            assert!(
                !scenario.objective.is_empty(),
                "objective must be non-empty for {}",
                scenario.id
            );
        }
    }

    #[test]
    fn benchmark_class_display_roundtrip() {
        let classes = [
            (BenchmarkClass::RepoExploration, "repo-exploration"),
            (BenchmarkClass::Documentation, "documentation"),
            (BenchmarkClass::SafeCodeChange, "safe-code-change"),
            (BenchmarkClass::SessionQuality, "session-quality"),
            (BenchmarkClass::TestWriting, "test-writing"),
            (BenchmarkClass::BugFix, "bug-fix"),
            (BenchmarkClass::Refactoring, "refactoring"),
            (BenchmarkClass::DependencyAnalysis, "dependency-analysis"),
            (BenchmarkClass::ErrorHandling, "error-handling"),
            (BenchmarkClass::PerformanceAnalysis, "performance-analysis"),
            (BenchmarkClass::SecurityAudit, "security-audit"),
            (BenchmarkClass::ApiDesign, "api-design"),
            (BenchmarkClass::CodeReview, "code-review"),
            (BenchmarkClass::Debugging, "debugging"),
            (BenchmarkClass::ConfigManagement, "config-management"),
            (BenchmarkClass::ConcurrencyAnalysis, "concurrency-analysis"),
            (BenchmarkClass::MigrationPlanning, "migration-planning"),
            (
                BenchmarkClass::ObservabilityInstrumentation,
                "observability-instrumentation",
            ),
            (BenchmarkClass::DataModeling, "data-modeling"),
            (BenchmarkClass::DataMigration, "data-migration"),
            (BenchmarkClass::CicdPipeline, "cicd-pipeline"),
            (BenchmarkClass::DependencyUpgrade, "dependency-upgrade"),
            (BenchmarkClass::ReleaseManagement, "release-management"),
            (BenchmarkClass::AccessibilityReview, "accessibility-review"),
            (
                BenchmarkClass::InternationalizationReview,
                "internationalization-review",
            ),
            (BenchmarkClass::IncidentResponse, "incident-response"),
            (
                BenchmarkClass::DatabaseSchemaChange,
                "database-schema-change",
            ),
            (BenchmarkClass::CachingStrategy, "caching-strategy"),
            (BenchmarkClass::FeatureFlagging, "feature-flagging"),
            (BenchmarkClass::RateLimiting, "rate-limiting"),
            (BenchmarkClass::EventSourcing, "event-sourcing"),
            (BenchmarkClass::ChaosEngineering, "chaos-engineering"),
        ];
        for (class, label) in classes {
            assert_eq!(class.to_string(), label);
        }
    }

    // --- resolve_benchmark_scenario: all scenarios resolve ---

    #[test]
    fn all_scenarios_resolve_by_id() {
        for scenario in benchmark_scenarios() {
            let resolved = resolve_benchmark_scenario(scenario.id).unwrap();
            assert_eq!(resolved.id, scenario.id);
            assert_eq!(resolved.class, scenario.class);
        }
    }

    // --- scenario consistency: identities match expected patterns ---

    #[test]
    fn all_scenarios_use_valid_identity() {
        for scenario in benchmark_scenarios() {
            assert!(
                scenario.identity == "simard-gym"
                    || scenario.identity == "simard-engineer"
                    || scenario.identity == "simard-composite-engineer",
                "unexpected identity '{}' in scenario '{}'",
                scenario.identity,
                scenario.id
            );
        }
    }

    #[test]
    fn all_scenarios_use_valid_base_type() {
        for scenario in benchmark_scenarios() {
            assert!(
                scenario.base_type == "local-harness"
                    || scenario.base_type == "terminal-shell"
                    || scenario.base_type == "copilot-sdk"
                    || scenario.base_type == "rusty-clawd",
                "unexpected base_type '{}' in scenario '{}'",
                scenario.base_type,
                scenario.id
            );
        }
    }

    // --- scenario ID format conventions ---

    #[test]
    fn all_scenario_ids_are_lowercase_kebab_case() {
        for scenario in benchmark_scenarios() {
            assert!(
                scenario
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit()),
                "scenario id '{}' must be lowercase kebab-case",
                scenario.id
            );
        }
    }

    #[test]
    fn all_scenarios_have_reasonable_evidence_requirements() {
        for scenario in benchmark_scenarios() {
            assert!(
                scenario.expected_min_runtime_evidence <= 20,
                "scenario '{}' requires too many evidence records: {}",
                scenario.id,
                scenario.expected_min_runtime_evidence
            );
        }
    }

    // --- BenchmarkClass: all 12 classes covered by at least one scenario ---

    #[test]
    fn every_benchmark_class_has_at_least_one_scenario() {
        let all_classes = [
            BenchmarkClass::RepoExploration,
            BenchmarkClass::Documentation,
            BenchmarkClass::SafeCodeChange,
            BenchmarkClass::SessionQuality,
            BenchmarkClass::TestWriting,
            BenchmarkClass::BugFix,
            BenchmarkClass::Refactoring,
            BenchmarkClass::DependencyAnalysis,
            BenchmarkClass::ErrorHandling,
            BenchmarkClass::PerformanceAnalysis,
            BenchmarkClass::SecurityAudit,
            BenchmarkClass::ApiDesign,
            BenchmarkClass::CodeReview,
            BenchmarkClass::Debugging,
            BenchmarkClass::ConfigManagement,
            BenchmarkClass::ConcurrencyAnalysis,
            BenchmarkClass::MigrationPlanning,
            BenchmarkClass::ObservabilityInstrumentation,
            BenchmarkClass::DataModeling,
            BenchmarkClass::DataMigration,
            BenchmarkClass::CicdPipeline,
            BenchmarkClass::DependencyUpgrade,
            BenchmarkClass::ReleaseManagement,
            BenchmarkClass::AccessibilityReview,
            BenchmarkClass::InternationalizationReview,
            BenchmarkClass::IncidentResponse,
            BenchmarkClass::DatabaseSchemaChange,
            BenchmarkClass::CachingStrategy,
            BenchmarkClass::FeatureFlagging,
            BenchmarkClass::RateLimiting,
            BenchmarkClass::EventSourcing,
            BenchmarkClass::ChaosEngineering,
        ];
        let scenarios = benchmark_scenarios();
        for class in all_classes {
            assert!(
                scenarios.iter().any(|s| s.class == class),
                "no scenario covers class '{}'",
                class
            );
        }
    }
}
