//! class_specific_checks helpers — chunk 3 of 6.

use super::super::types::BenchmarkCheckResult;

pub(super) fn checks_for_incident_response(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_release_management(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_performance_analysis(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_security_audit(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_safe_code_change(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_documentation(combined: &str) -> Vec<BenchmarkCheckResult> {
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
