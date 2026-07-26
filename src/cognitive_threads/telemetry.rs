//! The single telemetry seam for cognitive threads (Appendix A.6).
//!
//! All metric/span emission funnels through here. Each function dual-writes:
//! the structured `tracing` event it always emitted, AND — through the unified
//! [`crate::telemetry`] OTel facade — a real per-thread metric series so the
//! threads appear in `metrics_snapshot.json` (issue #4786). No
//! `println!`/`eprintln!` — structured `tracing` only. Metric/span **names**
//! use the fixed `simard.thread.<id>.<suffix>` scheme where `<id>` is a
//! per-thread compile-time constant (SR-11): untrusted content only ever
//! appears as length-bounded structured field *values*, never as a name.
//! Identity lives in the NAME (not an attribute), so every facade call passes an
//! EMPTY attribute set — see [`crate::telemetry::names`] for why (the
//! `MAX_VALUES_PER_KEY` cardinality cliff).

use super::thread::ThreadOutcome;
use crate::telemetry::{self, names};

/// Build a facade-ready metric name: `simard.thread.<id>.<suffix>`.
///
/// Thin wrapper over the single-source-of-truth [`names::thread_metric_name`]
/// (co-located with the scheme's constants); `id` and `suffix` are compile-time
/// constants at every call site (SR-11).
pub fn metric_name(id: &str, suffix: &str) -> String {
    names::thread_metric_name(id, suffix)
}

/// Record a completed run at `run_epoch` (Unix seconds): opens span
/// `simard.thread.<id>`, dual-writes the `runs` / `successes` | `failures`
/// counters, the `duration_seconds` histogram, and the `last_run_epoch` gauge,
/// and emits the structured event. `runs` counts every attempt; exactly one of
/// `successes` / `failures` is bumped so the success rate is derivable — the
/// failures counter is bumped HERE (not in [`record_error`]) so a failing tick
/// is counted exactly once.
pub fn record_run(id: &str, outcome: &ThreadOutcome, run_epoch: u64) {
    telemetry::counter_add(&metric_name(id, names::THREAD_SUFFIX_RUNS), 1, &[]);
    if outcome.success {
        telemetry::counter_add(&metric_name(id, names::THREAD_SUFFIX_SUCCESSES), 1, &[]);
    } else {
        telemetry::counter_add(&metric_name(id, names::THREAD_SUFFIX_FAILURES), 1, &[]);
    }
    telemetry::histogram_record(
        &metric_name(id, names::THREAD_SUFFIX_DURATION_SECONDS),
        outcome.duration.as_secs_f64(),
        &[],
    );
    telemetry::gauge_set(
        &metric_name(id, names::THREAD_SUFFIX_LAST_RUN_EPOCH),
        run_epoch as i64,
        &[],
    );

    let span = tracing::info_span!(
        "simard.thread",
        thread.id = id,
        ran = outcome.ran,
        success = outcome.success,
        duration_ms = outcome.duration.as_millis() as u64,
    );
    let _entered = span.enter();
    tracing::info!(
        metric = %metric_name(id, names::THREAD_SUFFIX_RUNS),
        thread.id = id,
        ran = outcome.ran,
        success = outcome.success,
        duration_seconds = outcome.duration.as_secs_f64(),
        run_epoch = run_epoch,
        summary = %outcome.summary,
        "cognitive thread run recorded"
    );
}

/// Emit an error-level structured event for a failed run. The `failures`
/// counter is NOT bumped here — [`record_run`] already counted this attempt as a
/// failure — so a single failed tick increments `failures` exactly once. This
/// keeps the human-readable error log distinct from the metric.
pub fn record_error(id: &str, reason: &str) {
    tracing::error!(
        metric = %metric_name(id, names::THREAD_SUFFIX_FAILURES),
        thread.id = id,
        reason = %reason,
        "cognitive thread run errored"
    );
}

/// Set the `simard.thread.<id>.next_run_epoch` gauge — the cadence / staleness
/// seam the Overseer reads. The gauge is written only when a next run is
/// computable (`Some`); an `OnDemand`/`EventDriven` thread has no cadence so no
/// gauge is set (its absence is honest, not a stale value).
pub fn record_next_run(id: &str, next_run_epoch: Option<u64>) {
    if let Some(epoch) = next_run_epoch {
        telemetry::gauge_set(
            &metric_name(id, names::THREAD_SUFFIX_NEXT_RUN_EPOCH),
            epoch as i64,
            &[],
        );
    }
    tracing::debug!(
        metric = %metric_name(id, names::THREAD_SUFFIX_NEXT_RUN_EPOCH),
        thread.id = id,
        next_run_epoch = ?next_run_epoch,
        "cognitive thread next-run scheduled"
    );
}

/// RAII guard: sets `simard.thread.<id>.active` to 1 while held, back to 0 on
/// drop.
#[must_use]
pub fn enter_active(id: &str) -> ActiveGuard {
    telemetry::gauge_set(&metric_name(id, names::THREAD_SUFFIX_ACTIVE), 1, &[]);
    tracing::debug!(
        metric = %metric_name(id, names::THREAD_SUFFIX_ACTIVE),
        thread.id = id,
        active = 1,
        "cognitive thread tick started"
    );
    ActiveGuard { id: id.to_string() }
}

/// Guard returned by [`enter_active`]; emits the `active = 0` signal on drop.
pub struct ActiveGuard {
    id: String,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        telemetry::gauge_set(&metric_name(&self.id, names::THREAD_SUFFIX_ACTIVE), 0, &[]);
        tracing::debug!(
            metric = %metric_name(&self.id, names::THREAD_SUFFIX_ACTIVE),
            thread.id = %self.id,
            active = 0,
            "cognitive thread tick finished"
        );
    }
}
