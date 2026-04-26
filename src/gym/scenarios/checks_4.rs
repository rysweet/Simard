//! class_specific_checks helpers — chunk 4 of 6.

use super::super::types::BenchmarkCheckResult;

pub(super) fn checks_for_database_schema_change(combined: &str) -> Vec<BenchmarkCheckResult> {
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

pub(super) fn checks_for_rate_limiting(combined: &str) -> Vec<BenchmarkCheckResult> {
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

pub(super) fn checks_for_api_design(combined: &str) -> Vec<BenchmarkCheckResult> {
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

pub(super) fn checks_for_refactoring(combined: &str) -> Vec<BenchmarkCheckResult> {
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

pub(super) fn checks_for_error_handling(combined: &str) -> Vec<BenchmarkCheckResult> {
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
