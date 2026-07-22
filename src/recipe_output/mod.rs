//! Shared recipe-runner-rs stdout parsing primitives + per-phase parse
//! observability counters (issue #2484).
//!
//! [`extract`] holds the hardened ANSI/log/banner stripping and JSON/verdict
//! extraction reused by every recipe-backed phase (distill, merge-judge,
//! engineer-lifecycle brain, decide, orient, progress-checker). This is the
//! one shared, well-tested path that replaces the formerly bespoke, fragile
//! per-phase extractors.

pub mod extract;

pub use extract::{
    VerdictMatch, balanced_objects, escape_json_string_control_chars,
    escape_json_string_invalid_escapes, extract_and_parse_json, extract_json_payload,
    extract_verdict, last_balanced_object, recover_json_view, strip_ansi,
    strip_json_trailing_commas, strip_recipe_noise,
};

/// Record the outcome of a recipe-output parse for one phase.
///
/// Emits `recipe_parse_success_total` (when `success`) or
/// `recipe_parse_failure_total` (when the phase fell back to its permissive
/// default) over the existing `metrics.jsonl` sink, tagging the `phase` in the
/// metric `context`. This gives both the numerator and the denominator so the
/// shared-extractor fix is measurable per phase.
///
/// `phase` ∈ {`distill`, `merge_judge`, `engineer_lifecycle`, `decide`,
/// `orient`, `progress_checker`}.
///
/// Best-effort and non-blocking: a metrics-sink error must never fail or
/// block real work, so the result is intentionally ignored.
pub fn record_parse_outcome(phase: &str, success: bool) {
    let metric_name = if success {
        "recipe_parse_success_total"
    } else {
        "recipe_parse_failure_total"
    };
    let _ = crate::self_metrics::record_metric(metric_name, 1.0, phase);
}
