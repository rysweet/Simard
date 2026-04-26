//! class_specific_checks helpers — chunk 2 of 6.

use super::super::types::BenchmarkCheckResult;

pub(super) fn checks_for_accessibility_review(combined: &str) -> Vec<BenchmarkCheckResult> {
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

pub(super) fn checks_for_cicd_pipeline(combined: &str) -> Vec<BenchmarkCheckResult> {
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

pub(super) fn checks_for_concurrency_analysis(combined: &str) -> Vec<BenchmarkCheckResult> {
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

pub(super) fn checks_for_migration_planning(combined: &str) -> Vec<BenchmarkCheckResult> {
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

pub(super) fn checks_for_code_review(combined: &str) -> Vec<BenchmarkCheckResult> {
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
