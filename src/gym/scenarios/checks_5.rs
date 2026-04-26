//! class_specific_checks helpers — chunk 5 of 6.

use super::super::types::BenchmarkCheckResult;

pub(super) fn checks_for_caching_strategy(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_chaos_engineering(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_observability_instrumentation(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_data_modeling(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_repo_exploration(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_session_quality(
    combined: &str,
    outcome: &crate::runtime::SessionOutcome,
    exported: &crate::handoff::RuntimeHandoffSnapshot,
) -> Vec<BenchmarkCheckResult> {
    let _ = combined;
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
