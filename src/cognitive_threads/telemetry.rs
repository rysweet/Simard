//! The single telemetry seam for cognitive threads (Appendix A.6).
//!
//! All metric/span emission funnels through here so a later rebase onto the
//! unified `src/telemetry/` OTel facade is a one-file change. No
//! `println!`/`eprintln!` — structured `tracing` only. Metric/span **names**
//! use the fixed `simard.thread.<id>.<suffix>` scheme where `<id>` is a
//! per-thread compile-time constant (SR-11): untrusted content only ever
//! appears as length-bounded structured field *values*, never as a name.
#![allow(dead_code)]

use super::thread::ThreadOutcome;

/// Build a facade-ready metric name: `simard.thread.<id>.<suffix>`.
///
/// `id` and `suffix` are compile-time constants at every call site (SR-11).
pub fn metric_name(id: &str, suffix: &str) -> String {
    format!("simard.thread.{id}.{suffix}")
}

/// Record a completed run: opens span `simard.thread.<id>` and emits the
/// `runs` / `duration_seconds` signals with outcome fields.
pub fn record_run(id: &str, outcome: &ThreadOutcome) {
    let span = tracing::info_span!(
        "simard.thread",
        thread.id = id,
        ran = outcome.ran,
        success = outcome.success,
        duration_ms = outcome.duration.as_millis() as u64,
    );
    let _entered = span.enter();
    tracing::info!(
        metric = %metric_name(id, "runs"),
        thread.id = id,
        ran = outcome.ran,
        success = outcome.success,
        duration_seconds = outcome.duration.as_secs_f64(),
        summary = %outcome.summary,
        "cognitive thread run recorded"
    );
}

/// Bump `simard.thread.<id>.errors` and emit an error-level structured event.
pub fn record_error(id: &str, reason: &str) {
    tracing::error!(
        metric = %metric_name(id, "errors"),
        thread.id = id,
        reason = %reason,
        "cognitive thread run errored"
    );
}

/// Set the `simard.thread.<id>.next_run_epoch` gauge.
pub fn record_next_run(id: &str, next_run_epoch: Option<u64>) {
    tracing::debug!(
        metric = %metric_name(id, "next_run_epoch"),
        thread.id = id,
        next_run_epoch = ?next_run_epoch,
        "cognitive thread next-run scheduled"
    );
}

/// RAII guard: sets `simard.thread.<id>.active` to 1 while held, back to 0 on
/// drop.
#[must_use]
pub fn enter_active(id: &str) -> ActiveGuard {
    tracing::debug!(
        metric = %metric_name(id, "active"),
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
        tracing::debug!(
            metric = %metric_name(&self.id, "active"),
            thread.id = %self.id,
            active = 0,
            "cognitive thread tick finished"
        );
    }
}
