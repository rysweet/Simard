//! class_specific_checks helpers — chunk 1 of 6.

use super::super::types::BenchmarkCheckResult;

pub(super) fn checks_for_internationalization_review(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_dependency_upgrade(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_data_migration(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_dependency_analysis(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_bug_fix(combined: &str) -> Vec<BenchmarkCheckResult> {
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
