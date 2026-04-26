//! class_specific_checks helpers — chunk 6 of 6.

use super::super::types::BenchmarkCheckResult;

pub(super) fn checks_for_feature_flagging(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_event_sourcing(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_debugging(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_config_management(combined: &str) -> Vec<BenchmarkCheckResult> {
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


pub(super) fn checks_for_test_writing(combined: &str) -> Vec<BenchmarkCheckResult> {
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
