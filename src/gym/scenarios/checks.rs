//! Class-specific check builders for `BenchmarkScenario`s.

use crate::handoff::RuntimeHandoffSnapshot;
use super::super::types::{BenchmarkCheckResult, BenchmarkClass, BenchmarkScenario};

/// Produce additional scenario-class-specific checks based on the scenario type.
///
/// These supplement the generic 8-check baseline with checks tailored to each
/// `BenchmarkClass`: structural discovery for repo exploration, doc validity
/// for documentation, compilation evidence for safe code changes, and test
/// structure for test writing scenarios.
pub(crate) fn class_specific_checks(
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
        BenchmarkClass::BugFix => {
            let defect_identified = combined.contains("bug")
                || combined.contains("defect")
                || combined.contains("issue")
                || combined.contains("unwrap")
                || combined.contains("expect")
                || combined.contains("panic");
            let fix_described = combined.contains("fix")
                || combined.contains("replac")
                || combined.contains("propagat")
                || combined.contains("convert")
                || combined.contains("refactor");
            let safety_analysis = combined.contains("safe")
                || combined.contains("error handling")
                || combined.contains("result")
                || combined.contains("graceful")
                || combined.contains("recover");
            vec![
                BenchmarkCheckResult {
                    id: "bug-defect-identified".to_string(),
                    passed: defect_identified,
                    detail: format!(
                        "execution output {} defect identification",
                        if defect_identified {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "bug-fix-described".to_string(),
                    passed: fix_described,
                    detail: format!(
                        "execution output {} fix description",
                        if fix_described { "includes" } else { "lacks" }
                    ),
                },
                BenchmarkCheckResult {
                    id: "bug-safety-analyzed".to_string(),
                    passed: safety_analysis,
                    detail: format!(
                        "execution output {} safety analysis",
                        if safety_analysis { "includes" } else { "lacks" }
                    ),
                },
            ]
        }
        BenchmarkClass::Refactoring => {
            let change_identified = combined.contains("extract")
                || combined.contains("simplif")
                || combined.contains("refactor")
                || combined.contains("renam")
                || combined.contains("restructur");
            let behavior_preserved = combined.contains("preserv")
                || combined.contains("behavior")
                || combined.contains("equivalent")
                || combined.contains("same result")
                || combined.contains("no change in");
            let code_shown = combined.contains("fn ")
                || combined.contains("before")
                || combined.contains("after")
                || combined.contains("original")
                || combined.contains("simplified");
            vec![
                BenchmarkCheckResult {
                    id: "refactor-change-identified".to_string(),
                    passed: change_identified,
                    detail: format!(
                        "execution output {} refactoring identification",
                        if change_identified {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "refactor-behavior-preserved".to_string(),
                    passed: behavior_preserved,
                    detail: format!(
                        "execution output {} behavior preservation evidence",
                        if behavior_preserved {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "refactor-code-shown".to_string(),
                    passed: code_shown,
                    detail: format!(
                        "execution output {} code examples",
                        if code_shown { "includes" } else { "lacks" }
                    ),
                },
            ]
        }
        BenchmarkClass::DependencyAnalysis => {
            let deps_analyzed = combined.contains("cargo.toml")
                || combined.contains("dependenc")
                || combined.contains("crate")
                || combined.contains("version");
            let coupling_assessed = combined.contains("import")
                || combined.contains("coupling")
                || combined.contains("module")
                || combined.contains("use crate");
            let recommendations_present = combined.contains("suggest")
                || combined.contains("recommend")
                || combined.contains("should")
                || combined.contains("consider")
                || combined.contains("audit");
            vec![
                BenchmarkCheckResult {
                    id: "dep-analysis-performed".to_string(),
                    passed: deps_analyzed,
                    detail: format!(
                        "execution output {} dependency analysis",
                        if deps_analyzed { "contains" } else { "lacks" }
                    ),
                },
                BenchmarkCheckResult {
                    id: "dep-coupling-assessed".to_string(),
                    passed: coupling_assessed,
                    detail: format!(
                        "execution output {} coupling assessment",
                        if coupling_assessed {
                            "contains"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "dep-recommendations-present".to_string(),
                    passed: recommendations_present,
                    detail: format!(
                        "execution output {} actionable recommendations",
                        if recommendations_present {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::ErrorHandling => {
            let error_analysis = combined.contains("unwrap")
                || combined.contains("error")
                || combined.contains("panic")
                || combined.contains("result");
            let classification_present = combined.contains("safe")
                || combined.contains("risky")
                || combined.contains("classif")
                || combined.contains("categor");
            let propagation_traced = combined.contains("propagat")
                || combined.contains("chain")
                || combined.contains("context")
                || combined.contains("diagnostic");
            vec![
                BenchmarkCheckResult {
                    id: "error-analysis-performed".to_string(),
                    passed: error_analysis,
                    detail: format!(
                        "execution output {} error analysis",
                        if error_analysis { "contains" } else { "lacks" }
                    ),
                },
                BenchmarkCheckResult {
                    id: "error-classification-present".to_string(),
                    passed: classification_present,
                    detail: format!(
                        "execution output {} error classification",
                        if classification_present {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "error-propagation-traced".to_string(),
                    passed: propagation_traced,
                    detail: format!(
                        "execution output {} propagation tracing",
                        if propagation_traced {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::PerformanceAnalysis => {
            let complexity_mentioned = combined.contains("o(n")
                || combined.contains("complexity")
                || combined.contains("quadratic")
                || combined.contains("linear")
                || combined.contains("big-o");
            let optimization_suggested = combined.contains("optimi")
                || combined.contains("cache")
                || combined.contains("memoiz")
                || combined.contains("allocat")
                || combined.contains("zero-copy");
            let bottleneck_identified = combined.contains("bottleneck")
                || combined.contains("hot path")
                || combined.contains("hot spot")
                || combined.contains("expensive")
                || combined.contains("repeated");
            vec![
                BenchmarkCheckResult {
                    id: "perf-complexity-analyzed".to_string(),
                    passed: complexity_mentioned,
                    detail: format!(
                        "execution output {} complexity analysis",
                        if complexity_mentioned {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "perf-optimization-suggested".to_string(),
                    passed: optimization_suggested,
                    detail: format!(
                        "execution output {} optimization suggestions",
                        if optimization_suggested {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "perf-bottleneck-identified".to_string(),
                    passed: bottleneck_identified,
                    detail: format!(
                        "execution output {} bottleneck identification",
                        if bottleneck_identified {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::SecurityAudit => {
            let vulnerability_found = combined.contains("unsafe")
                || combined.contains("vulnerab")
                || combined.contains("cve")
                || combined.contains("credential")
                || combined.contains("secret")
                || combined.contains("injection");
            let risk_assessed = combined.contains("risk")
                || combined.contains("severity")
                || combined.contains("low")
                || combined.contains("medium")
                || combined.contains("high")
                || combined.contains("critical");
            let remediation_proposed = combined.contains("remediat")
                || combined.contains("mitigat")
                || combined.contains("fix")
                || combined.contains("sanitiz")
                || combined.contains("validat");
            vec![
                BenchmarkCheckResult {
                    id: "security-vulnerability-found".to_string(),
                    passed: vulnerability_found,
                    detail: format!(
                        "execution output {} vulnerability identification",
                        if vulnerability_found {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "security-risk-assessed".to_string(),
                    passed: risk_assessed,
                    detail: format!(
                        "execution output {} risk assessment",
                        if risk_assessed { "includes" } else { "lacks" }
                    ),
                },
                BenchmarkCheckResult {
                    id: "security-remediation-proposed".to_string(),
                    passed: remediation_proposed,
                    detail: format!(
                        "execution output {} remediation proposal",
                        if remediation_proposed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::ApiDesign => {
            let api_surface_analyzed = combined.contains("pub fn")
                || combined.contains("pub struct")
                || combined.contains("pub trait")
                || combined.contains("public api")
                || combined.contains("api surface");
            let design_quality_assessed = combined.contains("ergonomic")
                || combined.contains("discoverab")
                || combined.contains("builder")
                || combined.contains("breaking change")
                || combined.contains("type safe");
            let recommendation_present = combined.contains("suggest")
                || combined.contains("recommend")
                || combined.contains("improv")
                || combined.contains("should")
                || combined.contains("consider");
            vec![
                BenchmarkCheckResult {
                    id: "api-surface-analyzed".to_string(),
                    passed: api_surface_analyzed,
                    detail: format!(
                        "execution output {} API surface analysis",
                        if api_surface_analyzed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "api-design-quality-assessed".to_string(),
                    passed: design_quality_assessed,
                    detail: format!(
                        "execution output {} design quality assessment",
                        if design_quality_assessed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "api-recommendation-present".to_string(),
                    passed: recommendation_present,
                    detail: format!(
                        "execution output {} design recommendations",
                        if recommendation_present {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::CodeReview => {
            let review_findings = combined.contains("finding")
                || combined.contains("issue")
                || combined.contains("concern")
                || combined.contains("inconsisten")
                || combined.contains("review");
            let severity_assessed = combined.contains("severity")
                || combined.contains("critical")
                || combined.contains("minor")
                || combined.contains("major")
                || combined.contains("nit");
            let fix_suggested = combined.contains("suggest")
                || combined.contains("recommend")
                || combined.contains("fix")
                || combined.contains("improv")
                || combined.contains("should");
            vec![
                BenchmarkCheckResult {
                    id: "review-findings-present".to_string(),
                    passed: review_findings,
                    detail: format!(
                        "execution output {} review findings",
                        if review_findings { "includes" } else { "lacks" }
                    ),
                },
                BenchmarkCheckResult {
                    id: "review-severity-assessed".to_string(),
                    passed: severity_assessed,
                    detail: format!(
                        "execution output {} severity assessment",
                        if severity_assessed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "review-fix-suggested".to_string(),
                    passed: fix_suggested,
                    detail: format!(
                        "execution output {} fix suggestions",
                        if fix_suggested { "includes" } else { "lacks" }
                    ),
                },
            ]
        }
        BenchmarkClass::Debugging => {
            let root_cause_traced = combined.contains("trace")
                || combined.contains("origin")
                || combined.contains("root cause")
                || combined.contains("source of")
                || combined.contains("caused by");
            let call_path_analyzed = combined.contains("call")
                || combined.contains("stack")
                || combined.contains("propagat")
                || combined.contains("invoked")
                || combined.contains("transition");
            let diagnostic_suggested = combined.contains("diagnostic")
                || combined.contains("debug")
                || combined.contains("log")
                || combined.contains("inspect")
                || combined.contains("breakpoint");
            vec![
                BenchmarkCheckResult {
                    id: "debug-root-cause-traced".to_string(),
                    passed: root_cause_traced,
                    detail: format!(
                        "execution output {} root cause tracing",
                        if root_cause_traced {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "debug-call-path-analyzed".to_string(),
                    passed: call_path_analyzed,
                    detail: format!(
                        "execution output {} call path analysis",
                        if call_path_analyzed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "debug-diagnostic-suggested".to_string(),
                    passed: diagnostic_suggested,
                    detail: format!(
                        "execution output {} diagnostic suggestions",
                        if diagnostic_suggested {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::ConfigManagement => {
            let config_inventoried = combined.contains("config")
                || combined.contains("feature")
                || combined.contains("env")
                || combined.contains("cargo.toml")
                || combined.contains("setting");
            let validation_checked = combined.contains("valid")
                || combined.contains("default")
                || combined.contains("missing")
                || combined.contains("required")
                || combined.contains("optional");
            let matrix_produced = combined.contains("matrix")
                || combined.contains("table")
                || combined.contains("inventory")
                || combined.contains("summary")
                || combined.contains("report");
            vec![
                BenchmarkCheckResult {
                    id: "config-inventoried".to_string(),
                    passed: config_inventoried,
                    detail: format!(
                        "execution output {} configuration inventory",
                        if config_inventoried {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "config-validation-checked".to_string(),
                    passed: validation_checked,
                    detail: format!(
                        "execution output {} validation assessment",
                        if validation_checked {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "config-matrix-produced".to_string(),
                    passed: matrix_produced,
                    detail: format!(
                        "execution output {} configuration matrix",
                        if matrix_produced { "includes" } else { "lacks" }
                    ),
                },
            ]
        }
        BenchmarkClass::ConcurrencyAnalysis => {
            let race_condition_analyzed = combined.contains("race")
                || combined.contains("concurrent")
                || combined.contains("shared")
                || combined.contains("mutex")
                || combined.contains("atomic");
            let synchronization_assessed = combined.contains("lock")
                || combined.contains("synchroniz")
                || combined.contains("rwlock")
                || combined.contains("channel")
                || combined.contains("arc");
            let safety_evaluated = combined.contains("deadlock")
                || combined.contains("safe")
                || combined.contains("cancel")
                || combined.contains("await")
                || combined.contains("spawn");
            vec![
                BenchmarkCheckResult {
                    id: "concurrency-race-analyzed".to_string(),
                    passed: race_condition_analyzed,
                    detail: format!(
                        "execution output {} race condition analysis",
                        if race_condition_analyzed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "concurrency-sync-assessed".to_string(),
                    passed: synchronization_assessed,
                    detail: format!(
                        "execution output {} synchronization assessment",
                        if synchronization_assessed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "concurrency-safety-evaluated".to_string(),
                    passed: safety_evaluated,
                    detail: format!(
                        "execution output {} concurrency safety evaluation",
                        if safety_evaluated {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::MigrationPlanning => {
            let migration_scope_defined = combined.contains("migrat")
                || combined.contains("schema")
                || combined.contains("version")
                || combined.contains("upgrade")
                || combined.contains("evolution");
            let compatibility_assessed = combined.contains("compat")
                || combined.contains("backward")
                || combined.contains("breaking")
                || combined.contains("deprecat")
                || combined.contains("serde");
            let plan_produced = combined.contains("step")
                || combined.contains("plan")
                || combined.contains("phase")
                || combined.contains("roadmap")
                || combined.contains("checkpoint");
            vec![
                BenchmarkCheckResult {
                    id: "migration-scope-defined".to_string(),
                    passed: migration_scope_defined,
                    detail: format!(
                        "execution output {} migration scope definition",
                        if migration_scope_defined {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "migration-compatibility-assessed".to_string(),
                    passed: compatibility_assessed,
                    detail: format!(
                        "execution output {} compatibility assessment",
                        if compatibility_assessed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "migration-plan-produced".to_string(),
                    passed: plan_produced,
                    detail: format!(
                        "execution output {} migration plan",
                        if plan_produced { "includes" } else { "lacks" }
                    ),
                },
            ]
        }
        BenchmarkClass::ObservabilityInstrumentation => {
            let instrumentation_analyzed = combined.contains("log")
                || combined.contains("trac")
                || combined.contains("metric")
                || combined.contains("instrument")
                || combined.contains("observab");
            let coverage_assessed = combined.contains("coverage")
                || combined.contains("gap")
                || combined.contains("missing")
                || combined.contains("module")
                || combined.contains("path");
            let recommendation_present = combined.contains("recommend")
                || combined.contains("suggest")
                || combined.contains("should")
                || combined.contains("add")
                || combined.contains("design");
            vec![
                BenchmarkCheckResult {
                    id: "observability-instrumentation-analyzed".to_string(),
                    passed: instrumentation_analyzed,
                    detail: format!(
                        "execution output {} instrumentation analysis",
                        if instrumentation_analyzed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "observability-coverage-assessed".to_string(),
                    passed: coverage_assessed,
                    detail: format!(
                        "execution output {} coverage assessment",
                        if coverage_assessed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "observability-recommendation-present".to_string(),
                    passed: recommendation_present,
                    detail: format!(
                        "execution output {} observability recommendations",
                        if recommendation_present {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::DataModeling => {
            let model_analyzed = combined.contains("type")
                || combined.contains("struct")
                || combined.contains("entity")
                || combined.contains("field")
                || combined.contains("schema");
            let relationships_mapped = combined.contains("relation")
                || combined.contains("reference")
                || combined.contains("owner")
                || combined.contains("contain")
                || combined.contains("cardinality");
            let quality_assessed = combined.contains("consisten")
                || combined.contains("safety")
                || combined.contains("invalid")
                || combined.contains("invariant")
                || combined.contains("newtype");
            vec![
                BenchmarkCheckResult {
                    id: "data-model-analyzed".to_string(),
                    passed: model_analyzed,
                    detail: format!(
                        "execution output {} data model analysis",
                        if model_analyzed { "includes" } else { "lacks" }
                    ),
                },
                BenchmarkCheckResult {
                    id: "data-relationships-mapped".to_string(),
                    passed: relationships_mapped,
                    detail: format!(
                        "execution output {} relationship mapping",
                        if relationships_mapped {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "data-quality-assessed".to_string(),
                    passed: quality_assessed,
                    detail: format!(
                        "execution output {} data quality assessment",
                        if quality_assessed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::DataMigration => {
            let schema_delta_described = combined.contains("schema")
                || combined.contains("field")
                || combined.contains("column")
                || combined.contains("version")
                || combined.contains("migrat");
            let compatibility_addressed = combined.contains("backward")
                || combined.contains("forward")
                || combined.contains("compat")
                || combined.contains("default")
                || combined.contains("optional")
                || combined.contains("serde");
            let rollout_or_rollback_planned = combined.contains("backfill")
                || combined.contains("rollout")
                || combined.contains("rollback")
                || combined.contains("phased")
                || combined.contains("revert")
                || combined.contains("compatibility window");
            vec![
                BenchmarkCheckResult {
                    id: "data-migration-schema-delta-described".to_string(),
                    passed: schema_delta_described,
                    detail: format!(
                        "execution output {} schema delta description",
                        if schema_delta_described {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "data-migration-compatibility-addressed".to_string(),
                    passed: compatibility_addressed,
                    detail: format!(
                        "execution output {} compatibility analysis",
                        if compatibility_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "data-migration-rollout-or-rollback-planned".to_string(),
                    passed: rollout_or_rollback_planned,
                    detail: format!(
                        "execution output {} rollout/rollback plan",
                        if rollout_or_rollback_planned {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::CicdPipeline => {
            let workflow_structure_described = combined.contains("workflow")
                || combined.contains("github actions")
                || combined.contains("job")
                || combined.contains("step")
                || combined.contains(".yml")
                || combined.contains(".yaml");
            let trigger_or_pin_addressed = combined.contains("trigger")
                || combined.contains("on:")
                || combined.contains("pull_request")
                || combined.contains("push")
                || combined.contains("pin")
                || combined.contains("uses:")
                || combined.contains("@v");
            let verification_or_remediation_present = combined.contains("cargo")
                || combined.contains("test")
                || combined.contains("check")
                || combined.contains("retry")
                || combined.contains("timeout")
                || combined.contains("cache")
                || combined.contains("matrix");
            vec![
                BenchmarkCheckResult {
                    id: "cicd-workflow-structure-described".to_string(),
                    passed: workflow_structure_described,
                    detail: format!(
                        "execution output {} workflow structure description",
                        if workflow_structure_described {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "cicd-trigger-or-pin-addressed".to_string(),
                    passed: trigger_or_pin_addressed,
                    detail: format!(
                        "execution output {} trigger/version-pin analysis",
                        if trigger_or_pin_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "cicd-verification-or-remediation-present".to_string(),
                    passed: verification_or_remediation_present,
                    detail: format!(
                        "execution output {} verification/remediation steps",
                        if verification_or_remediation_present {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::DependencyUpgrade => {
            let upgrade_target_named = combined.contains("cargo.toml")
                || combined.contains("dependenc")
                || combined.contains("crate")
                || combined.contains("version")
                || combined.contains("major");
            let breakage_analyzed = combined.contains("breaking")
                || combined.contains("breakage")
                || combined.contains("api change")
                || combined.contains("call site")
                || combined.contains("changelog")
                || combined.contains("deprecat");
            let verification_plan_present = combined.contains("cargo check")
                || combined.contains("cargo test")
                || combined.contains("verify")
                || combined.contains("regression")
                || combined.contains("rollout")
                || combined.contains("rollback")
                || combined.contains("staged");
            vec![
                BenchmarkCheckResult {
                    id: "dep-upgrade-target-named".to_string(),
                    passed: upgrade_target_named,
                    detail: format!(
                        "execution output {} upgrade target identification",
                        if upgrade_target_named {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "dep-upgrade-breakage-analyzed".to_string(),
                    passed: breakage_analyzed,
                    detail: format!(
                        "execution output {} breakage analysis",
                        if breakage_analyzed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "dep-upgrade-verification-plan-present".to_string(),
                    passed: verification_plan_present,
                    detail: format!(
                        "execution output {} verification/rollback plan",
                        if verification_plan_present {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::ReleaseManagement => {
            let version_bump_planned = combined.contains("version")
                || combined.contains("semver")
                || combined.contains("bump")
                || combined.contains("patch")
                || combined.contains("minor")
                || combined.contains("major");
            let changelog_authored = combined.contains("changelog")
                || combined.contains("release notes")
                || combined.contains("added")
                || combined.contains("changed")
                || combined.contains("fixed")
                || combined.contains("deprecat");
            let tag_or_cutover_addressed = combined.contains("tag")
                || combined.contains("git tag")
                || combined.contains("release")
                || combined.contains("cutover")
                || combined.contains("rollout")
                || combined.contains("rollback")
                || combined.contains("publish");
            vec![
                BenchmarkCheckResult {
                    id: "release-version-bump-planned".to_string(),
                    passed: version_bump_planned,
                    detail: format!(
                        "execution output {} version-bump plan",
                        if version_bump_planned {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "release-changelog-authored".to_string(),
                    passed: changelog_authored,
                    detail: format!(
                        "execution output {} changelog/release notes",
                        if changelog_authored {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "release-tag-or-cutover-addressed".to_string(),
                    passed: tag_or_cutover_addressed,
                    detail: format!(
                        "execution output {} tag/cutover plan",
                        if tag_or_cutover_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::AccessibilityReview => {
            let a11y_issues_identified = combined.contains("aria")
                || combined.contains("alt text")
                || combined.contains("alt-text")
                || combined.contains("label")
                || combined.contains("screen reader")
                || combined.contains("focus")
                || combined.contains("contrast")
                || combined.contains("keyboard");
            let wcag_or_standard_cited = combined.contains("wcag")
                || combined.contains("level a")
                || combined.contains("level aa")
                || combined.contains("level aaa")
                || combined.contains("success criterion")
                || combined.contains("1.1.1")
                || combined.contains("1.4.3")
                || combined.contains("1.4.11")
                || combined.contains("2.1.1")
                || combined.contains("2.4.3")
                || combined.contains("2.4.7")
                || combined.contains("4.1.2");
            let remediation_proposed = combined.contains("remediat")
                || combined.contains("fix")
                || combined.contains("add ")
                || combined.contains("replace")
                || combined.contains("suggest")
                || combined.contains("recommend")
                || combined.contains("improve");
            vec![
                BenchmarkCheckResult {
                    id: "a11y-issues-identified".to_string(),
                    passed: a11y_issues_identified,
                    detail: format!(
                        "execution output {} accessibility issue identification",
                        if a11y_issues_identified {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "a11y-wcag-cited".to_string(),
                    passed: wcag_or_standard_cited,
                    detail: format!(
                        "execution output {} WCAG/standard citation",
                        if wcag_or_standard_cited {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "a11y-remediation-proposed".to_string(),
                    passed: remediation_proposed,
                    detail: format!(
                        "execution output {} accessibility remediation",
                        if remediation_proposed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::InternationalizationReview => {
            let localizable_strings_identified = combined.contains("hardcoded")
                || combined.contains("string literal")
                || combined.contains("message catalog")
                || combined.contains("translat")
                || combined.contains("l10n")
                || combined.contains("i18n")
                || combined.contains("localiz")
                || combined.contains("message key");
            let locale_handling_described = combined.contains("locale")
                || combined.contains("language tag")
                || combined.contains("bcp 47")
                || combined.contains("bcp-47")
                || combined.contains("accept-language")
                || combined.contains("fallback")
                || combined.contains("cldr")
                || combined.contains("en-us")
                || combined.contains("pt-br")
                || combined.contains("region");
            let pluralization_or_format_addressed = combined.contains("plural")
                || combined.contains("rtl")
                || combined.contains("bidi")
                || combined.contains("date format")
                || combined.contains("number format")
                || combined.contains("currency")
                || combined.contains("icu")
                || combined.contains("messageformat")
                || combined.contains("fluent")
                || combined.contains("gettext");
            vec![
                BenchmarkCheckResult {
                    id: "i18n-localizable-strings-identified".to_string(),
                    passed: localizable_strings_identified,
                    detail: format!(
                        "execution output {} localizable string identification",
                        if localizable_strings_identified {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "i18n-locale-handling-described".to_string(),
                    passed: locale_handling_described,
                    detail: format!(
                        "execution output {} locale-handling description",
                        if locale_handling_described {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "i18n-pluralization-or-format-addressed".to_string(),
                    passed: pluralization_or_format_addressed,
                    detail: format!(
                        "execution output {} pluralization/format coverage",
                        if pluralization_or_format_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::IncidentResponse => {
            let timeline_reconstructed = combined.contains("timeline")
                || combined.contains("sequence")
                || combined.contains("when ")
                || combined.contains("started at")
                || combined.contains("alert")
                || combined.contains("paged")
                || combined.contains("detected")
                || combined.contains("resolved at");
            let root_cause_or_contributing_identified = combined.contains("root cause")
                || combined.contains("root-cause")
                || combined.contains("contributing")
                || combined.contains("trigger")
                || combined.contains("cascade")
                || combined.contains("fault")
                || combined.contains("latent")
                || combined.contains("blameless");
            let mitigation_or_followup_proposed = combined.contains("mitigat")
                || combined.contains("action item")
                || combined.contains("follow-up")
                || combined.contains("followup")
                || combined.contains("runbook")
                || combined.contains("postmortem")
                || combined.contains("post-mortem")
                || combined.contains("prevention")
                || combined.contains("escalation")
                || combined.contains("on-call")
                || combined.contains("oncall");
            vec![
                BenchmarkCheckResult {
                    id: "incident-timeline-reconstructed".to_string(),
                    passed: timeline_reconstructed,
                    detail: format!(
                        "execution output {} incident timeline reconstruction",
                        if timeline_reconstructed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "incident-root-cause-or-contributing-identified".to_string(),
                    passed: root_cause_or_contributing_identified,
                    detail: format!(
                        "execution output {} root cause/contributing factor analysis",
                        if root_cause_or_contributing_identified {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "incident-mitigation-or-followup-proposed".to_string(),
                    passed: mitigation_or_followup_proposed,
                    detail: format!(
                        "execution output {} mitigation/follow-up proposal",
                        if mitigation_or_followup_proposed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::DatabaseSchemaChange => {
            let migration_plan_described = combined.contains("migration")
                || combined.contains("schema change")
                || combined.contains("ddl")
                || combined.contains("alter table")
                || combined.contains("add column")
                || combined.contains("drop column")
                || combined.contains("create index");
            let compatibility_addressed = combined.contains("backward")
                || combined.contains("compatib")
                || combined.contains("expand/contract")
                || combined.contains("expand-contract")
                || combined.contains("dual-write")
                || combined.contains("dual write")
                || combined.contains("dual-read")
                || combined.contains("nullable")
                || combined.contains("default value")
                || combined.contains("phase");
            let rollback_or_safety_addressed = combined.contains("rollback")
                || combined.contains("revert")
                || combined.contains("backfill")
                || combined.contains("online")
                || combined.contains("concurrently")
                || combined.contains("downtime")
                || combined.contains("lock")
                || combined.contains("replica");
            vec![
                BenchmarkCheckResult {
                    id: "schema-migration-plan-described".to_string(),
                    passed: migration_plan_described,
                    detail: format!(
                        "execution output {} schema migration plan",
                        if migration_plan_described {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "schema-compatibility-addressed".to_string(),
                    passed: compatibility_addressed,
                    detail: format!(
                        "execution output {} compatibility/phasing discussion",
                        if compatibility_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "schema-rollback-or-safety-addressed".to_string(),
                    passed: rollback_or_safety_addressed,
                    detail: format!(
                        "execution output {} rollback/safety considerations",
                        if rollback_or_safety_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::CachingStrategy => {
            let cache_pattern_named = combined.contains("cache-aside")
                || combined.contains("cache aside")
                || combined.contains("write-through")
                || combined.contains("write through")
                || combined.contains("write-back")
                || combined.contains("read-through")
                || combined.contains("lazy load")
                || combined.contains("lazy-load");
            let invalidation_or_ttl_addressed = combined.contains("ttl")
                || combined.contains("invalidat")
                || combined.contains("eviction")
                || combined.contains("expir")
                || combined.contains("staleness")
                || combined.contains("freshness")
                || combined.contains("epoch")
                || combined.contains("version");
            let consistency_or_stampede_addressed = combined.contains("stampede")
                || combined.contains("thundering")
                || combined.contains("singleflight")
                || combined.contains("coalescing")
                || combined.contains("consisten")
                || combined.contains("hit rate")
                || combined.contains("hit ratio")
                || combined.contains("miss");
            vec![
                BenchmarkCheckResult {
                    id: "cache-pattern-named".to_string(),
                    passed: cache_pattern_named,
                    detail: format!(
                        "execution output {} a named caching pattern",
                        if cache_pattern_named {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "cache-invalidation-or-ttl-addressed".to_string(),
                    passed: invalidation_or_ttl_addressed,
                    detail: format!(
                        "execution output {} invalidation/TTL discussion",
                        if invalidation_or_ttl_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "cache-consistency-or-stampede-addressed".to_string(),
                    passed: consistency_or_stampede_addressed,
                    detail: format!(
                        "execution output {} consistency/stampede discussion",
                        if consistency_or_stampede_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::FeatureFlagging => {
            let flag_mechanism_described = combined.contains("feature flag")
                || combined.contains("feature-flag")
                || combined.contains("toggle")
                || combined.contains("kill switch")
                || combined.contains("kill-switch")
                || combined.contains("flag store")
                || combined.contains("rollout");
            let rollout_or_cohort_addressed = combined.contains("percentage")
                || combined.contains("cohort")
                || combined.contains("bucket")
                || combined.contains("ramp")
                || combined.contains("gradual")
                || combined.contains("a/b")
                || combined.contains("experiment")
                || combined.contains("hash");
            let safety_or_default_addressed = combined.contains("default")
                || combined.contains("fail-safe")
                || combined.contains("fail safe")
                || combined.contains("fail-open")
                || combined.contains("fail-closed")
                || combined.contains("rollback")
                || combined.contains("guardrail")
                || combined.contains("audit");
            vec![
                BenchmarkCheckResult {
                    id: "feature-flag-mechanism-described".to_string(),
                    passed: flag_mechanism_described,
                    detail: format!(
                        "execution output {} feature flag mechanism",
                        if flag_mechanism_described {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "feature-flag-rollout-or-cohort-addressed".to_string(),
                    passed: rollout_or_cohort_addressed,
                    detail: format!(
                        "execution output {} rollout/cohort strategy",
                        if rollout_or_cohort_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "feature-flag-safety-or-default-addressed".to_string(),
                    passed: safety_or_default_addressed,
                    detail: format!(
                        "execution output {} safe-default/guardrail discussion",
                        if safety_or_default_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::RateLimiting => {
            let algorithm_named = combined.contains("token bucket")
                || combined.contains("token-bucket")
                || combined.contains("leaky bucket")
                || combined.contains("leaky-bucket")
                || combined.contains("sliding window")
                || combined.contains("sliding-window")
                || combined.contains("fixed window")
                || combined.contains("fixed-window");
            let limits_or_quota_addressed = combined.contains("rps")
                || combined.contains("requests per")
                || combined.contains("quota")
                || combined.contains("burst")
                || combined.contains("capacity")
                || combined.contains("refill")
                || combined.contains("per-key")
                || combined.contains("per key");
            let rejection_or_distribution_addressed = combined.contains("429")
                || combined.contains("retry-after")
                || combined.contains("retry after")
                || combined.contains("x-ratelimit")
                || combined.contains("backoff")
                || combined.contains("reject")
                || combined.contains("distribut")
                || combined.contains("redis")
                || combined.contains("central");
            vec![
                BenchmarkCheckResult {
                    id: "rate-limit-algorithm-named".to_string(),
                    passed: algorithm_named,
                    detail: format!(
                        "execution output {} a named rate-limit algorithm",
                        if algorithm_named { "includes" } else { "lacks" }
                    ),
                },
                BenchmarkCheckResult {
                    id: "rate-limit-limits-or-quota-addressed".to_string(),
                    passed: limits_or_quota_addressed,
                    detail: format!(
                        "execution output {} concrete limits/quota parameters",
                        if limits_or_quota_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "rate-limit-rejection-or-distribution-addressed".to_string(),
                    passed: rejection_or_distribution_addressed,
                    detail: format!(
                        "execution output {} rejection/distribution handling",
                        if rejection_or_distribution_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::EventSourcing => {
            let event_store_described = combined.contains("event store")
                || combined.contains("event-store")
                || combined.contains("event log")
                || combined.contains("event-log")
                || combined.contains("append-only")
                || combined.contains("append only")
                || combined.contains("aggregate")
                || combined.contains("event sourc");
            let projection_or_replay_addressed = combined.contains("projection")
                || combined.contains("read model")
                || combined.contains("read-model")
                || combined.contains("replay")
                || combined.contains("rebuild")
                || combined.contains("catch-up")
                || combined.contains("catch up");
            let consistency_or_versioning_addressed = combined.contains("idempot")
                || combined.contains("sequence")
                || combined.contains("ordering")
                || combined.contains("schema version")
                || combined.contains("upcast")
                || combined.contains("checkpoint")
                || combined.contains("optimistic concurrency")
                || combined.contains("snapshot");
            vec![
                BenchmarkCheckResult {
                    id: "event-store-described".to_string(),
                    passed: event_store_described,
                    detail: format!(
                        "execution output {} event store/log description",
                        if event_store_described {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "event-projection-or-replay-addressed".to_string(),
                    passed: projection_or_replay_addressed,
                    detail: format!(
                        "execution output {} projection/replay discussion",
                        if projection_or_replay_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "event-consistency-or-versioning-addressed".to_string(),
                    passed: consistency_or_versioning_addressed,
                    detail: format!(
                        "execution output {} consistency/versioning discussion",
                        if consistency_or_versioning_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
        BenchmarkClass::ChaosEngineering => {
            let experiment_or_fault_described = combined.contains("chaos")
                || combined.contains("fault injection")
                || combined.contains("fault-injection")
                || combined.contains("game day")
                || combined.contains("game-day")
                || combined.contains("experiment")
                || combined.contains("inject");
            let blast_radius_addressed = combined.contains("blast radius")
                || combined.contains("blast-radius")
                || combined.contains("scope")
                || combined.contains("subset")
                || combined.contains("canary")
                || combined.contains("minimum viable")
                || combined.contains("limit");
            let hypothesis_or_safety_addressed = combined.contains("hypothesis")
                || combined.contains("steady state")
                || combined.contains("steady-state")
                || combined.contains("abort")
                || combined.contains("rollback")
                || combined.contains("guardrail")
                || combined.contains("kill switch")
                || combined.contains("kill-switch")
                || combined.contains("threshold");
            vec![
                BenchmarkCheckResult {
                    id: "chaos-experiment-or-fault-described".to_string(),
                    passed: experiment_or_fault_described,
                    detail: format!(
                        "execution output {} chaos experiment/fault description",
                        if experiment_or_fault_described {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "chaos-blast-radius-addressed".to_string(),
                    passed: blast_radius_addressed,
                    detail: format!(
                        "execution output {} blast-radius/scope discussion",
                        if blast_radius_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
                BenchmarkCheckResult {
                    id: "chaos-hypothesis-or-safety-addressed".to_string(),
                    passed: hypothesis_or_safety_addressed,
                    detail: format!(
                        "execution output {} hypothesis/abort/safety discussion",
                        if hypothesis_or_safety_addressed {
                            "includes"
                        } else {
                            "lacks"
                        }
                    ),
                },
            ]
        }
    }
}

