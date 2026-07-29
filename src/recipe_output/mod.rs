//! Shared recipe-runner-rs stdout sanitization primitives and per-phase parse
//! observability counters (issue #2484).
//!
//! [`extract`] provides the retained [`strip_ansi`] and [`strip_recipe_noise`]
//! APIs. They remove ANSI escapes and known recipe-runner, logging, and launcher
//! noise while preserving clean output through a zero-allocation borrowed path.

pub mod extract;

pub use extract::{strip_ansi, strip_recipe_noise};

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
